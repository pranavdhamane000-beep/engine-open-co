//! WP02 — Immutable assets and content manifest (spec §11).
//!
//! Content-addressed asset storage: BLAKE3 hashes the original stored source
//! bytes, object identity is content, and identical content may share one
//! immutable object. Assets are displayed metadata; original filenames never
//! determine object paths. A failed import cleans temporary data and leaves
//! the project unchanged; temporary files are never resolvable as objects.

pub mod audio;
pub mod decode;
pub mod hash;
pub mod image;
pub mod import;
pub mod manifest;
pub mod resolver;
pub mod store;

use thiserror::Error;

pub use hash::ContentHash;
pub use import::import;
pub use manifest::{manifest_digest, AssetKind, AssetManifest, AssetManifestEntry};
pub use resolver::{resolve, verify, AssetDiagnostic, AssetResolver, ResolvedAsset};
pub use store::AssetStore;

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AssetError {
    #[error("io: {0}")]
    Io(String),
    #[error("content hash mismatch for object {0}")]
    HashMismatch(String),
    #[error("size mismatch: expected {expected} B, found {found} B")]
    SizeMismatch { expected: u64, found: u64 },
    #[error("invalid content hash: {0}")]
    InvalidHash(String),
    #[error("asset id {0} already exists with different content")]
    DuplicateAssetId(String),
    #[error("asset id {0} not found in manifest")]
    MissingAssetId(String),
    #[error("object for {0} missing from store")]
    MissingObject(String),
    #[error("no decoder registered for kind {0:?} at importer version {1}")]
    UnsupportedDecoder(String, u32),
    #[error("dependency cycle detected at asset {0}")]
    DependencyCycle(String),
    #[error("decode failed: {0}")]
    Decode(String),
    #[error("operation cancelled")]
    Cancelled,
}

pub type AssetResult<T> = Result<T, AssetError>;

/// Map a std::io error into the message-carrying Io variant.
pub(crate) fn io_err(ctx: &str, e: std::io::Error) -> AssetError {
    AssetError::Io(format!("{ctx}: {e}"))
}
