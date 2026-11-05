//! Win32 flush/replace implementation (spec 10.3) — the exact sequencing the
//! WP00 replacement probe verified on this machine (ADR-0004):
//! write temp → sync_all → `ReplaceFileW(dest, temp, backup)` with the temp
//! file on the same volume (beside the destination).
//!
//! Failure ordering matters: anything that fails BEFORE the replace call
//! leaves the destination untouched with its complete old contents; the
//! replace commit itself is atomic.

use crate::error::CoreError;
use crate::io::atomic_replace::io_err;
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

/// Encode a path as NUL-terminated UTF-16 for Win32 wide APIs.
fn wide(p: &Path) -> Vec<u16> {
    let mut s: Vec<u16> = p.as_os_str().encode_wide().collect();
    s.push(0);
    s
}

pub fn replace_file(dest: &Path, payload: &[u8]) -> Result<(), CoreError> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{ReplaceFileW, REPLACE_FILE_FLAGS};

    let file_name = dest
        .file_name()
        .ok_or_else(|| CoreError::Io("destination has no file name".into()))?;
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    let _ = file_name;

    // Same-volume temporary + backup beside the destination (spec 9.6).
    let unique = uuid::Uuid::new_v4().hyphenated().to_string();
    let name = file_name.to_string_lossy();
    let tmp = parent.join(format!("{name}.{unique}.new.tmp"));
    let backup = parent.join(format!("{name}.{unique}.old.tmp"));

    // Phase 1 — complete new state to the temp file, flushed to the medium.
    let phase1 = (|| -> Result<(), CoreError> {
        let mut f = std::fs::File::create(&tmp).map_err(|e| io_err("temp create", e))?;
        f.write_all(payload).map_err(|e| io_err("temp write", e))?;
        f.sync_all().map_err(|e| io_err("temp flush", e))?;
        Ok(())
    })();
    if let Err(e) = phase1 {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    // Phase 2 — atomic commit. BackupPath receives the OLD contents; if no
    // backup file exists yet Windows creates it. Destination untouched on
    // any pre-commit failure (WP00 probe, hash-verified).
    let result = unsafe {
        ReplaceFileW(
            PCWSTR(wide(dest).as_ptr()),
            PCWSTR(wide(&tmp).as_ptr()),
            PCWSTR(wide(&backup).as_ptr()),
            REPLACE_FILE_FLAGS(0),
            None,
            None,
        )
    };

    match result {
        Ok(()) => {}
        Err(err) => {
            // Pre-commit failure: destination keeps its complete old bytes.
            let _ = std::fs::remove_file(&tmp);
            return Err(CoreError::Io(format!("ReplaceFileW (commit): {err}")));
        }
    }

    // Phase 3 — reopen-verify: destination must hold exactly the payload.
    let verified = std::fs::read(dest)
        .map_err(|e| io_err("reopen after commit", e))
        .and_then(|back| {
            (back == payload).then_some(()).ok_or_else(|| {
                CoreError::Io("reopen-verify failed: destination bytes differ".into())
            })
        });
    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_file(&backup);
    verified
}

/// Recovery-aware atomic write (spec 12.7) used by WP03 project saves:
/// temp `create_new` → write+flush+sync → ReplaceFileW
/// (REPLACEFILE_WRITE_THROUGH, backup KEPT) or MoveFileExW first-save →
/// reopen-verify → best-effort directory flush with explicit warning.
pub fn replace_file_reported(
    dest: &Path,
    payload: &[u8],
) -> Result<crate::io::atomic_replace::CommittedFile, CoreError> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, ReplaceFileW, MOVEFILE_WRITE_THROUGH, REPLACEFILE_WRITE_THROUGH,
        REPLACE_FILE_FLAGS,
    };

    let file_name = dest
        .file_name()
        .ok_or_else(|| CoreError::Io("destination has no file name".into()))?;
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));

    // Phase 1 — unique same-volume temporary beside the destination,
    // create_new semantics (fail if the uuid collides).
    let unique = uuid::Uuid::new_v4().hyphenated().to_string();
    let name = file_name.to_string_lossy();
    let tmp = parent.join(format!("{name}.{unique}.tmp"));
    let backup = parent.join(format!("{name}.{unique}.old.tmp"));

    let phase1 = (|| -> Result<(), CoreError> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| io_err("temp create_new", e))?;
        f.write_all(payload).map_err(|e| io_err("temp write", e))?;
        f.flush().map_err(|e| io_err("temp flush", e))?;
        f.sync_all().map_err(|e| io_err("temp sync", e))?;
        Ok(())
    })();
    if let Err(e) = phase1 {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    // Phase 2 — atomic commit.
    let dest_existed = dest.exists();
    let commit_result = if dest_existed {
        unsafe {
            ReplaceFileW(
                PCWSTR(wide(dest).as_ptr()),
                PCWSTR(wide(&tmp).as_ptr()),
                PCWSTR(wide(&backup).as_ptr()),
                REPLACE_FILE_FLAGS(REPLACEFILE_WRITE_THROUGH.0),
                None,
                None,
            )
        }
        .map(|_| ())
    } else {
        unsafe {
            MoveFileExW(
                PCWSTR(wide(&tmp).as_ptr()),
                PCWSTR(wide(dest).as_ptr()),
                MOVEFILE_WRITE_THROUGH,
            )
        }
        .map(|_| ())
    };
    if let Err(err) = commit_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(CoreError::Io(format!("save commit: {err}")));
    }

    // Phase 3 — reopen-verify (destination holds exactly the payload).
    let verified = std::fs::read(dest)
        .map_err(|e| io_err("reopen after commit", e))
        .and_then(|back| {
            (back == payload).then_some(()).ok_or_else(|| {
                CoreError::Io("reopen-verify failed: destination bytes differ".into())
            })
        });
    if verified.is_err() {
        let _ = std::fs::remove_file(&tmp);
        return Err(verified.err().unwrap());
    }

    // tmp was MOVED into place on first-save; otherwise it remains a copy.
    if dest_existed {
        let _ = std::fs::remove_file(&tmp);
    }

    // Phase 4 — attempt directory flush; report unsupported durability.
    let durability_warning = crate::io::atomic_replace::flush_directory(dest).err();

    Ok(crate::io::atomic_replace::CommittedFile {
        backup: dest_existed.then_some(backup),
        durability_warning,
    })
}

/// Attempt to flush the parent DIRECTORY through a backup-semantics handle.
/// Windows does not support FlushFileBuffers on directory handles; that
/// failure is returned as a durability warning, never swallowed silently.
pub fn flush_parent_directory(dest: &Path) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FlushFileBuffers, FILE_FLAG_BACKUP_SEMANTICS, FILE_GENERIC_WRITE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide(parent).as_ptr()),
            FILE_GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    };
    match handle {
        Ok(h) => {
            let flushed = unsafe { FlushFileBuffers(h) };
            let _ = unsafe { CloseHandle(h) };
            match flushed {
                Ok(_) => Ok(()),
                Err(e) => Err(format!(
                    "directory flush unsupported (contents intact, filesystem cache may delay durability): {e}"
                )),
            }
        }
        Err(e) => {
            Err(format!("directory handle unavailable for flush (contents intact): {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_replaces_completely() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("project.anim");
        std::fs::write(&dest, b"old-state").unwrap();
        replace_file(&dest, b"new-complete-state").unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"new-complete-state");
        // no stray .tmp artifacts remain
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x == "tmp").unwrap_or(false))
            .collect();
        assert!(leftovers.is_empty(), "leftovers: {leftovers:?}");
    }

    #[test]
    fn replace_against_missing_destination_fails_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("absent.anim");
        let res = replace_file(&missing, b"payload");
        assert!(res.is_err(), "replace against missing dest must fail");
        assert!(!missing.exists(), "must not fabricate the destination");
    }
}
