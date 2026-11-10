//! anim_editor — Editor Integration and Windows Ink (spec §19, WP10).
//!
//! The editor never mutates project/scene storage directly: every edit is a
//! command envelope through the WP03 single-writer boundary, every read is an
//! immutable `Arc<EditorSnapshot>`. Ink strokes preview transiently and
//! commit exactly once; adaptive quality is confined to interactive preview
//! and never reaches project or export settings.

#![deny(clippy::all)]
#![allow(clippy::multiple_crate_versions)] // wgpu/egui ecosystem transitive dup minors; revisit WP11

pub mod error;
pub mod ink;
pub mod preview_quality;
pub mod session;
pub mod transport;

pub use anim_commands::CommandError;

pub use error::EditorError;
pub use ink::{
    ActiveStroke, CommitSpec, InkEffect, InkEngine, PenPoint, PointerEvent,
    PointerEventKind, RawPoint, StrokePayloadV1, StrokeSetup, StrokeState, Tool,
    MAX_STROKE_POINTS,
};
pub use preview_quality::{
    PreviewQuality, PreviewQualityController, QualityScale, EMA_ALPHA,
    OVER_BUDGET_DOWNGRADE_FRAMES, UNDER_BUDGET_FRACTION, UNDER_BUDGET_UPGRADE_FRAMES,
};
pub use session::EditorSession;
pub use transport::{EditorTransport, TransportSnapshot};

/// Client-supplied job id for an export request (spec 19.9 `JobId`).
pub type JobId = uuid::Uuid;

/// Immutable editor snapshot (spec 19.6: the UI reads these, never scene
/// storage). Composed by the session after every applied transaction.
#[derive(Debug, Clone, PartialEq)]
pub struct EditorSnapshot {
    pub project_revision: anim_core::Revision,
    pub transport: TransportSnapshot,
    pub quality: PreviewQuality,
    pub undo_depth: usize,
    pub redo_depth: usize,
    pub last_command_id: Option<anim_core::CommandId>,
    pub last_transaction_label: Option<String>,
}

/// Durable receipt for an accepted envelope.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandReceipt {
    pub command_id: anim_core::CommandId,
    pub new_revision: anim_core::Revision,
}

/// Editor -> command boundary (spec 19.9). Implementations MUST be the WP03
/// single-writer processor; nothing else mutates project/scene storage.
pub trait CommandSink {
    fn issue(
        &self,
        command: anim_commands::CommandEnvelope,
    ) -> Result<CommandReceipt, CommandError>;
}

/// Immutable snapshot read for panels/viewport (spec 19.9).
pub trait SnapshotReader {
    fn current(&self) -> std::sync::Arc<EditorSnapshot>;
}
