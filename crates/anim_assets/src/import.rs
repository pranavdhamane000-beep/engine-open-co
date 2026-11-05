//! Streaming transactional source import (spec 11.3, 11.5 steps 3–4, 11.7).
//!
//! Sequence proven by contract:
//!   1. unique temporary file in the SAME-STORE staging dir (same volume)
//!   2. read bounded chunks; update BLAKE3 and a checked byte count
//!   3. write, flush, sync_all
//!   4. derive final path from the VERIFIED hash; if the object already
//!      exists, verify its length+hash and discard the temp (identical
//!      content shares one immutable object)
//!   5. otherwise rename WITHOUT overwrite — a competing commit can never
//!      be clobbered, and a failure before commit leaves the store intact
//!   6. return AssetManifestEntry ONLY after persistence.
//!
//! A failed import cleans temporary data and leaves the project unchanged
//! (spec 11.11); a manifest never references a temporary file.

use crate::hash::{ContentHash, StreamingHasher};
use crate::manifest::AssetManifestEntry;
use crate::store::{AssetStore, STREAM_CHUNK};
use crate::{io_err, AssetError, AssetKind, AssetResult};
use anim_core::id::AssetId;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub fn import<R: Read>(
    store: &AssetStore,
    id: AssetId,
    name: String,
    kind: AssetKind,
    importer_version: u32,
    source: R,
) -> AssetResult<AssetManifestEntry> {
    store.ensure_layout()?;
    let tmp = store.create_unique_temp()?;

    // 1–3: stream to the staging file while hashing and counting
    let outcome = stage_source(&tmp, source, importer_version);

    let (hash, byte_length) = match outcome {
        Ok((h, n)) => (h, n),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp); // failed import leaves no objects
            return Err(e);
        }
    };

    let final_path = store.object_path(&hash);

    // spec 11.7: create parent directories, then rename without overwrite
    if let Some(parent) = final_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            crate::io_err("create object parent dirs", e)
        })?;
    }

    // 4–5: nonoverwriting rename commit; identical content → dedup
    match commit(&tmp, &final_path) {
        Ok(()) => {}
        Err(CommitError::DestinationExists) => {
            // Object already exists: verify its length+hash BEFORE discarding
            // the temp (spec 11.7). Same immutable content, one object.
            let existing = entry_from(hash, byte_length, id, name, kind, importer_version)?;
            match store.verify(&existing) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&tmp);
                    return Ok(existing);
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(e);
                }
            }
        }
        Err(CommitError::Failed(e)) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    }

    entry_from(hash, byte_length, id, name, kind, importer_version)
}

fn entry_from(
    hash: ContentHash,
    byte_length: u64,
    id: AssetId,
    name: String,
    kind: AssetKind,
    importer_version: u32,
) -> AssetResult<AssetManifestEntry> {
    // Construction validates dependency invariants; here there are none.
    // The contract still refuses to panic (spec 11.11): map any
    // invariant-violation internally into a typed error.
    AssetManifestEntry::new(id, hash, byte_length, kind, importer_version, Vec::new(), name)
        .map_err(|e| {
            crate::AssetError::Io(format!("internal entry invariant violated: {e}"))
        })
}

fn stage_source<R: Read>(
    tmp: &Path,
    mut source: R,
    _importer_version: u32,
) -> AssetResult<(ContentHash, u64)> {
    let mut file =
        std::fs::File::create(tmp).map_err(|e| io_err("create staging file", e))?;
    let mut hasher = StreamingHasher::new();
    let mut buf = vec![0u8; STREAM_CHUNK];
    let mut total: u64 = 0;
    loop {
        match source.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                hasher.update(&buf[..n]);
                file.write_all(&buf[..n]).map_err(|e| io_err("staging write", e))?;
                total = total.checked_add(n as u64).ok_or_else(|| {
                    AssetError::Io("byte count overflow while staging".into())
                })?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(io_err("source read", e)),
        }
    }
    // flush + sync to the medium BEFORE commit (spec 11.7 "flush, synchronize")
    file.flush().map_err(|e| io_err("staging flush", e))?;
    file.sync_all().map_err(|e| io_err("staging sync", e))?;
    drop(file);
    Ok((hasher.finalize(), total))
}

enum CommitError {
    DestinationExists,
    Failed(crate::AssetError),
}

/// Rename WITHOUT overwrite (spec 11.7). On Windows: `MoveFileExW` with no
/// REPLACE_EXISTING flag — fails ERROR_ALREADY_EXISTS (183)/access errors if
/// the destination exists, so a racing commit can never be clobbered.
fn commit(tmp: &Path, final_path: &Path) -> Result<(), CommitError> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};
        let wide = |p: &Path| -> Vec<u16> {
            let mut v: Vec<u16> = p.as_os_str().encode_wide().collect();
            v.push(0);
            v
        };
        let result = unsafe {
            MoveFileExW(
                windows::core::PCWSTR(wide(tmp).as_ptr()),
                windows::core::PCWSTR(wide(final_path).as_ptr()),
                MOVEFILE_WRITE_THROUGH,
            )
        };
        match result {
            Ok(()) => Ok(()),
            Err(e) => {
                let h = e.code().0 as u32;
                // ERROR_ALREADY_EXISTS / ERROR_FILE_EXISTS / ERROR_ACCESS_DENIED
                // all mean a destination collision — the dedup case (verify
                // existing object, discard temp).
                if [0x8007_00B7u32, 0x8007_0050, 0x8007_0005].contains(&h) {
                    return Err(CommitError::DestinationExists);
                }
                Err(CommitError::Failed(crate::AssetError::Io(format!(
                    "rename commit failed: {e}"
                ))))
            }
        }
    }
    #[cfg(unix)]
    {
        // std::fs::rename is atomic but REPLACES on unix — guard explicitly.
        if final_path.exists() {
            return Err(CommitError::DestinationExists);
        }
        std::fs::rename(tmp, final_path).map_err(|e| {
            if final_path.exists() {
                CommitError::DestinationExists
            } else {
                CommitError::Failed(crate::io_err("rename commit", e))
            }
        })
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = tmp;
        let _ = final_path;
        Err(CommitError::Failed(crate::AssetError::Io(
            "unsupported platform for atomic object commit".into(),
        )))
    }
}

/// Re-exported path type usage for resolvers.
pub type StoredPath = PathBuf;
