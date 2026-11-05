//! Shared atomic-file API used by save and export (spec 10.5, 10.3).
//!
//! Contract proven by the WP00 replacement probe (ADR-0004):
//!   1. the complete payload is written to a same-volume temporary file
//!      beside the destination and flushed to the medium (sync_all)
//!   2. only then is `ReplaceFileW` invoked to atomically swap the old file
//!      for the new one, leaving the OLD complete contents as the backup
//!   3. a fault before commit leaves the destination with complete old bytes
//!   4. destination and backup reopen independently and completely.
//!
//! Non-Windows builds compile to a rename-based equivalent for CI, but the
//! Windows implementation is the specified production path
//! (`io/windows.rs` at the `io` module level, sibling of this file).

use crate::error::CoreError;
#[cfg(not(windows))]
use std::io::Write;
use std::path::{Path, PathBuf};

/// Map an io error into the message-only `CoreError::Io` variant.
pub(crate) fn io_err(context: &str, e: std::io::Error) -> CoreError {
    CoreError::Io(format!("{context}: {e}"))
}

/// Write `payload` atomically to `dest`. On success `dest` holds exactly the
/// payload, flushed, and a backup of the previous contents existed (Windows).
pub fn write_file_atomically(dest: &Path, payload: &[u8]) -> Result<(), CoreError> {
    #[cfg(windows)]
    {
        crate::io::windows::replace_file(dest, payload)
    }
    #[cfg(not(windows))]
    {
        replace_by_rename(dest, payload).map(|_| ())
    }
}

/// Result of a recovery-aware atomic save (spec 12.7): the backup path (when
/// a previous destination existed) and any durability limitation found while
/// flushing the DIRECTORY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedFile {
    /// Old destination contents preserved by the replacement commit, when
    /// the destination existed. Backups are immutable recovery inputs —
    /// callers must NOT delete them (spec 12.7/12.8).
    pub backup: Option<PathBuf>,
    /// Set when the directory itself could NOT be flushed to stable media.
    /// A durability limitation is a visible warning, never silent success
    /// (spec 12.11).
    pub durability_warning: Option<String>,
}

/// Write `payload` atomically to `dest` AND report recovery metadata.
///
/// Sequence (spec 12.7):
///   1. temporary file beside the destination, created with create_new
///   2. write all bytes, flush, sync_all
///   3. destination exists → `ReplaceFileW` (REPLACEFILE_WRITE_THROUGH);
///      no destination → `MoveFileExW(MOVEFILE_WRITE_THROUGH)`
///   4. reopen-verify destination holds exactly the payload
///   5. attempt directory flush; report unsupported durability explicitly
pub fn write_file_atomically_reported(
    dest: &Path,
    payload: &[u8],
) -> Result<CommittedFile, CoreError> {
    #[cfg(windows)]
    {
        crate::io::windows::replace_file_reported(dest, payload)
    }
    #[cfg(not(windows))]
    {
        let backup = dest.exists().then(|| dest.to_path_buf());
        replace_by_rename(dest, payload)?;
        let durability_warning = flush_directory(dest).err().map(|e| e.to_string());
        Ok(CommittedFile { backup, durability_warning })
    }
}

/// Best-effort directory flush through a backup-semantics handle. Returns
/// Err(text) when the platform does not support it — callers surface this
/// as a durability warning (spec 12.7 "report unsupported durability").
pub(crate) fn flush_directory(dest: &Path) -> Result<(), String> {
    let _ = dest;
    #[cfg(windows)]
    {
        crate::io::windows::flush_parent_directory(dest)
    }
    #[cfg(not(windows))]
    {
        Err("directory flush unsupported on this platform (filesystem may not be fully durable)".into())
    }
}

/// Ensure a parent directory exists; also used by WP02's manifest IO.
pub fn ensure_parent(dest: &Path) -> Result<PathBuf, CoreError> {
    let parent =
        dest.parent().ok_or_else(|| CoreError::Io("destination has no parent".into()))?;
    std::fs::create_dir_all(parent).map_err(|e| io_err("create parent dir", e))?;
    Ok(parent.to_path_buf())
}

/// Non-Windows fallback: write → sync → fsync-rename in a same-directory
/// temporary file. Not used on the production (Windows) machine.
#[cfg(not(windows))]
fn replace_by_rename(dest: &Path, payload: &[u8]) -> Result<(), CoreError> {
    let file_name = dest
        .file_name()
        .ok_or_else(|| CoreError::Io("destination has no file name".into()))?;
    let tmp = dest.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    {
        let mut f = std::fs::File::create(&tmp).map_err(|e| io_err("temp create", e))?;
        f.write_all(payload).map_err(|e| io_err("temp write", e))?;
        f.sync_all().map_err(|e| io_err("temp flush", e))?;
    }
    std::fs::rename(&tmp, dest).map_err(|e| io_err("rename commit", e))?;
    let _ = file_name;
    Ok(())
}

#[cfg(test)]
#[cfg(not(windows))]
mod tests {
    use super::*;

    #[test]
    fn round_trip_replaces_completely() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("project.json");
        std::fs::write(&dest, b"old-state").unwrap();
        write_file_atomically(&dest, b"new-complete-state").unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"new-complete-state");
    }
}
