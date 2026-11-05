//! WP03 — the authoritative project document (spec §12).
//!
//! `ProjectDocument` is the single persistent state object: it owns the
//! asset manifest (WP02) and the raw scenes/nodes/properties. It contains
//! NO editor, renderer, device or undo-stack references (spec 12.8).
//!
//! Dependency direction (spec 12.4): anim_project → anim_assets → anim_core.
//! No dependency on evaluation, rendering or editor crates.

pub mod document;
pub mod migrate;
pub mod property;
pub mod recovery;
pub mod save;
pub mod scene;
pub mod schema;

use thiserror::Error;

pub use anim_assets::{
    resolve as resolve_asset, AssetKind, AssetManifest, AssetManifestEntry, ContentHash,
};
pub use anim_core::{
    AssetId, FramePos, FrameRange, FrameRate, NodeId, ProjectId, Revision, SceneId, TrackId,
};

pub use document::{ProjectDocument, PROJECT_SCHEMA_VERSION};
pub use migrate::migrate_document;
pub use property::{Easing, PropertyKey, PropertyTrack, PropertyValue};
pub use recovery::{
    scan_recovery, select_recovery, RecoveryCandidate, RecoveryDecision, RecoveryKind,
};
pub use save::{
    autosave_path_for, load_autosave, save_atomic, save_atomic_staged, save_autosave,
    SaveReport, SaveStage, AUTOSAVE_SUFFIX,
};
pub use scene::{NodeData, SceneData, SceneMarker, TrackData};
pub use schema::{
    canonical_json, document_hash, load_document, parse_document, validate_document,
};

/// Document-layer errors. `MigrationFailure` and `Persistence` are spec
/// §12.11 surfaces of THIS crate; command-layer errors (revision conflicts,
/// replay mismatch, …) live in `anim_commands::CommandError`.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProjectError {
    #[error("unsupported schema version {found} (supported: {supported})")]
    UnsupportedSchema { found: u32, supported: u32 },
    #[error("validation: {0}")]
    Validation(String),
    #[error("migration failure: {0}")]
    Migration(String),
    #[error("persistence: {0}")]
    Persistence(String),
    #[error("serialization: {0}")]
    Serialize(String),
    #[error("recovery: {0}")]
    Recovery(String),
    #[error("io: {0}")]
    Io(String),
}

impl From<anim_core::error::CoreError> for ProjectError {
    fn from(e: anim_core::error::CoreError) -> Self {
        ProjectError::Io(e.to_string())
    }
}

pub type ProjectResult<T> = Result<T, ProjectError>;
