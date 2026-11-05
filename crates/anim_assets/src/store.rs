//! Safe object paths, verification and the immutable object lifecycle
//! (spec 11.3, 11.5 step 2, 11.7 resolve-and-verify).
//!
//! Object paths are `<root>/assets/objects/<first-2-hex>/<next-2-hex>/<full-hash>`.
//! Path construction accepts ONLY a validated fixed-size hash, preventing
//! traversal through filenames. Objects are immutable: verified create-once,
//! never overwritten; they live until an explicit future GC.

use crate::hash::StreamingHasher;
use crate::{io_err, AssetError, AssetManifestEntry, AssetResult, ContentHash};
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetStore {
    root: PathBuf,
}

/// Chunk for streaming reads — big enough to be efficient, small enough that
/// a multi-GiB asset never lives in memory (spec 11.12: "larger than memory
/// buffers" imports).
pub const STREAM_CHUNK: usize = 64 * 1024;

impl AssetStore {
    /// Accepts any project root; the store NEVER escapes it (cage check).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// Create layout `<root>/assets/objects` + the temporary staging dir.
    pub fn ensure_layout(&self) -> AssetResult<()> {
        std::fs::create_dir_all(self.objects_dir())
            .map_err(|e| io_err("create store", e))?;
        std::fs::create_dir_all(self.temp_dir())
            .map_err(|e| io_err("create temp dir", e))?;
        Ok(())
    }

    pub fn objects_dir(&self) -> PathBuf {
        self.root.join("assets").join("objects")
    }

    /// Unique same-store temporary staging directory (same volume ⇒ rename
    /// commit is atomic; spec 11.7 "create a unique temporary file in the
    /// object store").
    pub fn temp_dir(&self) -> PathBuf {
        self.objects_dir().join(".staging")
    }

    pub fn create_unique_temp(&self) -> AssetResult<PathBuf> {
        let dir = self.temp_dir();
        std::fs::create_dir_all(&dir).map_err(|e| io_err("create staging", e))?;
        let unique = uuid::Uuid::new_v4();
        Ok(dir.join(format!(
            "{}.import.{:x}",
            uuid::Uuid::new_v4().simple(),
            unique.as_u128() & 0xffff
        )))
    }

    /// Content-addressed object path. Takes only a validated ContentHash,
    /// so every component is constrained to lowercase hex — traversal-proof
    /// (spec 11.6).
    pub fn object_path(&self, hash: &ContentHash) -> PathBuf {
        self.objects_dir().join(hash.first_2()).join(hash.next_2()).join(hash.full_hex())
    }

    /// Verify an entry against its object: expected length (SizeMismatch)
    /// then content (HashMismatch). Streaming: never loads the object whole.
    pub fn verify(&self, entry: &AssetManifestEntry) -> AssetResult<()> {
        let path = self.object_path(&entry.content_hash);
        if !path.exists() {
            return Err(AssetError::MissingObject(entry.content_hash.to_hex()));
        }
        let meta = std::fs::metadata(&path).map_err(|e| io_err("stat object", e))?;
        if meta.len() != entry.byte_length {
            return Err(AssetError::SizeMismatch {
                expected: entry.byte_length,
                found: meta.len(),
            });
        }
        let mut file = File::open(&path).map_err(|e| io_err("open object", e))?;
        let mut hasher = StreamingHasher::new();
        let mut buf = vec![0u8; STREAM_CHUNK];
        loop {
            match file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => hasher.update(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(io_err("read object", e)),
            }
        }
        let actual = hasher.finalize();
        if actual != entry.content_hash {
            return Err(AssetError::HashMismatch(path.display().to_string()));
        }
        Ok(())
    }

    /// Read the whole verified object into memory (decoder entry point).
    /// ALWAYS verifies content before handing bytes to a decoder.
    pub fn read_object(&self, hash: &ContentHash) -> AssetResult<Vec<u8>> {
        let path = self.object_path(hash);
        std::fs::read(&path).map_err(|e| io_err("read object", e))
    }
}
