//! Autosave selection and validation (spec 12.3, 12.7 Recovery).
//!
//! Recovery candidates are scanned, and each is validated INDEPENDENTLY:
//! only a schema-valid complete document with a matching project ID and an
//! explicitly recorded revision qualifies. The scan NEVER deletes anything
//! — parse errors preserve all candidate files for diagnosis (spec 12.11).
//!
//! Selection rule:
//!   - manual lineage = current + backup files
//!   - the valid candidate with the HIGHEST revision is selected; current
//!     wins ties against its own backup
//!   - an autosave NEVER silently overwrites a newer manual save: if an
//!     autosave's revision is ≤ the best manual revision it is reported as
//!     ignored; if it is newer the lineage is ambiguous and the CHOICE is
//!     presented to the user (never auto-selected)

use crate::save::{autosave_path_for, parse_autosave_bytes, BACKUP_SUFFIX};
use crate::{ProjectDocument, ProjectResult};
use anim_core::ProjectId;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RecoveryKind {
    Current,
    Backup,
    Autosave,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecoveryCandidate {
    pub path: PathBuf,
    pub kind: RecoveryKind,
    /// Recorded revision when schema-valid; None when unparseable.
    pub revision: Option<u64>,
    pub project_id: Option<String>,
    /// true = schema-valid complete doc AND matching project id.
    pub valid: bool,
    /// Diagnostic detail when invalid (never a silent drop).
    pub detail: String,
}

/// Scan a directory for current/backup/autosave candidates of `base_name`
/// and validate each independently.
pub fn scan_recovery(
    project_id: ProjectId,
    directory: &Path,
    base_name: &str,
) -> ProjectResult<Vec<RecoveryCandidate>> {
    let mut out = Vec::new();

    // current file
    let current = directory.join(base_name);
    if current.exists() {
        out.push(assess(project_id, &current, RecoveryKind::Current)?);
    }

    // backups: `<base_name>.<uuid>.old.tmp` (spec replace-commit naming)
    let mut backups: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(directory) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&format!("{base_name}.")) && name.ends_with(BACKUP_SUFFIX) {
                backups.push(entry.path());
            }
        }
    }
    backups.sort();
    for backup in backups {
        out.push(assess(project_id, &backup, RecoveryKind::Backup)?);
    }

    // autosave
    let autosave = autosave_path_for(&current);
    if autosave.exists() {
        out.push(assess(project_id, &autosave, RecoveryKind::Autosave)?);
    }

    Ok(out)
}

fn assess(
    project_id: ProjectId,
    path: &Path,
    kind: RecoveryKind,
) -> ProjectResult<RecoveryCandidate> {
    let mut candidate = RecoveryCandidate {
        path: path.to_path_buf(),
        kind,
        revision: None,
        project_id: None,
        valid: false,
        detail: String::new(),
    };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            candidate.detail = format!("unreadable: {e}");
            return Ok(candidate);
        }
    };
    match load_candidate_document(kind, &bytes) {
        Ok(doc) => {
            candidate.revision = Some(doc.revision.as_u64());
            let pid = doc.project_id.to_string();
            candidate.project_id = Some(pid.clone());
            if doc.project_id == project_id {
                candidate.valid = true;
            } else {
                candidate.detail =
                    format!("project id mismatch: file has {pid}, expected {project_id}");
            }
        }
        Err(e) => {
            // PARSE ERRORS PRESERVE THE FILE — diagnostic only (spec 12.11).
            candidate.detail = format!("invalid document: {e}");
        }
    }
    Ok(candidate)
}

fn load_candidate_document(
    kind: RecoveryKind,
    bytes: &[u8],
) -> ProjectResult<ProjectDocument> {
    if kind == RecoveryKind::Autosave {
        parse_autosave_bytes(bytes)
    } else {
        crate::schema::parse_document(bytes)
    }
}

/// The user-choice-preserving recovery selection (see module docs).
#[derive(Debug, Clone, PartialEq)]
pub enum RecoveryDecision {
    Selected(RecoveryCandidate),
    /// Autosave is NEWER than every valid manual candidate — recovery
    /// lineage is ambiguous; the caller MUST present the choice (spec 12.7
    /// "present recovery choices when lineage is ambiguous").
    Ambiguous {
        candidates: Vec<RecoveryCandidate>,
    },
    /// No recoverable candidate at all.
    None,
}

pub fn select_recovery(candidates: &[RecoveryCandidate]) -> RecoveryDecision {
    let valid: Vec<&RecoveryCandidate> = candidates.iter().filter(|c| c.valid).collect();

    let best_manual = valid
        .iter()
        .filter(|c| c.kind != RecoveryKind::Autosave)
        .map(|c| c.revision.unwrap_or(0))
        .max();
    let best_autosave = valid
        .iter()
        .filter(|c| c.kind == RecoveryKind::Autosave)
        .map(|c| c.revision.unwrap_or(0))
        .max();

    if let (Some(auto_rev), Some(manual_rev)) = (best_autosave, best_manual) {
        if auto_rev > manual_rev {
            // NEVER silently overwrite a newer manual save with an older
            // autosave — and an autosave NEWER than the manual lineage is
            // exactly the ambiguous case: present the choice.
            return RecoveryDecision::Ambiguous { candidates: candidates.to_vec() };
        }
    }

    // Selection among valid manual candidates (autosave already judged ≤
    // best manual, so it can never be chosen here).
    let mut ranked: Vec<&RecoveryCandidate> =
        valid.into_iter().filter(|c| c.kind != RecoveryKind::Autosave).collect();
    ranked.sort_by_key(|c| {
        std::cmp::Reverse((c.revision.unwrap_or(0), c.kind == RecoveryKind::Current))
    });

    match ranked.first() {
        Some(c) => RecoveryDecision::Selected((*c).clone()),
        None => RecoveryDecision::None,
    }
}
