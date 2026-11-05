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
        replace_by_rename(dest, payload)
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
