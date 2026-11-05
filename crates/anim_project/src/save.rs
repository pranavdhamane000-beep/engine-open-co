//! Save orchestration and recovery metadata (spec 12.3, 12.7).
//!
//! Sequence proven by contract (§12.7 Atomic save):
//!   1. validate + serialize BEFORE touching disk — original destination
//!      preserved on any serialize/write/flush/replacement failure
//!   2. unique `<name>.<uuid>.tmp` beside the destination, create_new
//!   3. write all bytes, flush, sync_all
//!   4. destination exists → ReplaceFileW(REPLACEFILE_WRITE_THROUGH),
//!      backup holds the OLD complete contents and is KEPT (recovery input)
//!      no destination   → MoveFileExW(MOVEFILE_WRITE_THROUGH)
//!   5. reopen-verify destination holds exactly the new bytes
//!   6. attempt directory flush; unsupported durability is a visible
//!      WARNING, never silent success
//!
//! The document's "dirty state" is owned by the caller: a successful
//! SaveReport is the only signal on which it may be cleared (the document
//! itself holds no editor state per spec 12.8).

use crate::schema::canonical_json;
use crate::{ProjectDocument, ProjectError, ProjectResult};
use anim_core::Revision;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Suffix for autosave files beside the destination.
pub const AUTOSAVE_SUFFIX: &str = ".autosave.json";
/// Backup files carry this suffix (immutables recovery inputs, spec 12.8).
pub const BACKUP_SUFFIX: &str = ".old.tmp";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveReport {
    pub destination: PathBuf,
    /// Old contents preserved by the replacement commit. `None` when the
    /// destination did not exist (first save — nothing to preserve).
    pub backup: Option<PathBuf>,
    pub revision: Revision,
    /// BLAKE3 of the canonical document bytes just committed.
    pub document_hash: String,
    /// Directory-flush durability warning (spec 12.7: report unsupported
    /// durability explicitly).
    pub durability_warning: Option<String>,
}

/// Atomic save of the authoritative document (spec 12.7).
pub fn save_atomic(
    document: &ProjectDocument,
    destination: &Path,
) -> ProjectResult<SaveReport> {
    // 1. validate + serialize BEFORE touching disk (invalid documents can
    //    never reach the medium, and a failed write preserves the original).
    let canonical = canonical_json(document)?;
    let hash = {
        let mut hasher = anim_assets::hash::StreamingHasher::new();
        for chunk in canonical.chunks(anim_assets::store::STREAM_CHUNK) {
            hasher.update(chunk);
        }
        hasher.finalize()
    };

    // 2–6. shared, probe-proven implementation (spec 12.4).
    let committed = anim_core::io::write_file_atomically_reported(destination, &canonical)
        .map_err(|e| ProjectError::Persistence(format!("save commit: {e}")))?;

    Ok(SaveReport {
        destination: destination.to_path_buf(),
        backup: committed.backup,
        revision: document.revision,
        document_hash: hash.full_hex(),
        durability_warning: committed.durability_warning,
    })
}

/// Autosave envelope: wall-clock metadata lives OUTSIDE the authoritative
/// document (spec 12.8 — the document carries no editor/session state),
/// wrapping an unmodified `ProjectDocument`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutosaveEnvelope {
    pub autosave_format_version: u32,
    /// Session wall-clock (Unix seconds). NOT part of the document hash.
    pub saved_at_unix: u64,
    pub document: ProjectDocument,
}

pub const AUTOSAVE_FORMAT_VERSION: u32 = 1;

/// Write an autosave file beside the destination atomically. Returns the
/// autosave path.
pub fn save_autosave(
    document: &ProjectDocument,
    destination: &Path,
) -> ProjectResult<PathBuf> {
    // complete + valid BEFORE disk (same rule as save_atomic)
    let canonical = canonical_json(document)?;
    let doc: ProjectDocument = serde_json::from_slice(&canonical)
        .map_err(|e| ProjectError::Serialize(e.to_string()))?;
    let envelope = AutosaveEnvelope {
        autosave_format_version: AUTOSAVE_FORMAT_VERSION,
        saved_at_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        document: doc,
    };
    let bytes = serde_json::to_vec(&envelope)
        .map_err(|e| ProjectError::Serialize(format!("autosave serialize: {e}")))?;
    let autosave_path = autosave_path_for(destination);
    // reported variant handles BOTH first-save (MoveFileExW) and replace
    // (ReplaceFileW keeping backup); old autosave contents are not part of
    // the manual-save recovery lineage, so a superseded autosave's backup
    // copy is discarded immediately.
    let committed =
        anim_core::io::write_file_atomically_reported(&autosave_path, &bytes)
            .map_err(|e| ProjectError::Persistence(format!("autosave commit: {e}")))?;
    if let Some(old) = &committed.backup {
        let _ = std::fs::remove_file(old);
    }
    Ok(autosave_path)
}

/// The autosave file path for a destination: `<destination>.autosave.json`.
pub fn autosave_path_for(destination: &Path) -> PathBuf {
    let mut s = destination.as_os_str().to_owned();
    s.push(AUTOSAVE_SUFFIX);
    PathBuf::from(s)
}

/// Save-stage breakpoints for FAULT INJECTION (spec 12.12 "inject failures
/// at every command and persistence boundary; kill test processes at each
/// save stage"). Each stop reproduces the filesystem state a process KILLED
/// at that point would leave: no cleanup, destination untouched where the
/// commit has not happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveStage {
    /// Validation + serialization done; disk untouched (nothing to recover).
    Serialized,
    /// Temp file created, written and synced; NO commit — the true
    /// pre-replacement kill state (old destination complete, a staging
    /// .tmp leftover remains on disk, exactly like a killed process).
    TempWritten,
}

/// Stage-stopped save for fault-injection tests/ops drills. Errors early
/// like the real path (invalid documents never touch disk); every stop is
/// an injected error WITHOUT the normal cleanup, so subsequent recovery
/// and save behavior can be verified against genuine crash debris.
///
/// This is a TEST/OPS surface: production callers use `save_atomic`.
pub fn save_atomic_staged(
    document: &ProjectDocument,
    destination: &Path,
    stop_after: SaveStage,
) -> ProjectResult<SaveReport> {
    // 1. validate + serialize (invalid documents NEVER touch disk)
    let canonical = canonical_json(document)?;
    let mut hasher = anim_assets::hash::StreamingHasher::new();
    for chunk in canonical.chunks(anim_assets::store::STREAM_CHUNK) {
        hasher.update(chunk);
    }
    let hash = hasher.finalize();
    if stop_after == SaveStage::Serialized {
        return Err(ProjectError::Persistence("injected stop after Serialized".into()));
    }

    // 2. unique temp file beside the destination (create_new), synced
    let file_name = destination
        .file_name()
        .ok_or_else(|| ProjectError::Persistence("destination has no file name".into()))?;
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let unique = uuid::Uuid::new_v4().hyphenated().to_string();
    let tmp = parent.join(format!("{}.{unique}.tmp", file_name.to_string_lossy()));
    let staged = (|| -> ProjectResult<()> {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| ProjectError::Persistence(format!("temp create_new: {e}")))?;
        f.write_all(&canonical)
            .map_err(|e| ProjectError::Persistence(format!("temp write: {e}")))?;
        f.flush().map_err(|e| ProjectError::Persistence(format!("temp flush: {e}")))?;
        f.sync_all().map_err(|e| ProjectError::Persistence(format!("temp sync: {e}")))?;
        Ok(())
    })();
    if let Err(e) = staged {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    if stop_after == SaveStage::TempWritten {
        // kill simulation: temp file LEFT in place (crash debris)
        return Err(ProjectError::Persistence("injected stop after TempWritten".into()));
    }

    // 3–6 normal save (validation/serialization already done)
    let committed = anim_core::io::write_file_atomically_reported(destination, &canonical)
        .map_err(|e| ProjectError::Persistence(format!("save commit: {e}")))?;
    Ok(SaveReport {
        destination: destination.to_path_buf(),
        backup: committed.backup,
        revision: document.revision,
        document_hash: hash.full_hex(),
        durability_warning: committed.durability_warning,
    })
}

/// Load + validate autosave contents (the recovery input path).
pub fn load_autosave(path: &Path) -> ProjectResult<ProjectDocument> {
    let bytes =
        std::fs::read(path).map_err(|e| ProjectError::Io(format!("read autosave: {e}")))?;
    parse_autosave_bytes(&bytes)
}

pub(crate) fn parse_autosave_bytes(bytes: &[u8]) -> ProjectResult<ProjectDocument> {
    let env: AutosaveEnvelope = serde_json::from_slice(bytes)
        .map_err(|e| ProjectError::Recovery(format!("autosave envelope: {e}")))?;
    if env.autosave_format_version != AUTOSAVE_FORMAT_VERSION {
        return Err(ProjectError::UnsupportedSchema {
            found: env.autosave_format_version,
            supported: AUTOSAVE_FORMAT_VERSION,
        });
    }
    crate::schema::parse_document(
        &serde_json::to_vec(&env.document)
            .map_err(|e| ProjectError::Serialize(e.to_string()))?,
    )
}
