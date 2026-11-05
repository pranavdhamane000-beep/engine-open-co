//! WP03 — commands, undo, atomic project persistence (spec §12).
//!
//! The `CommandProcessor` is the SINGLE MUTATION BOUNDARY for editor, CLI
//! and future automation. UI and headless callers submit IDENTICAL
//! envelopes; the LLM director later uses the same registry, dry-run and
//! revision checks and receives no privileged mutation route (spec 12.10).
//!
//! Dependency direction (spec 12.4): anim_commands → anim_project
//! (→ anim_assets → anim_core). No rendering/editor/evaluation deps.

pub mod command;
pub mod dry_run;
pub mod envelope;
pub mod history;
pub mod models;
pub mod processor;
pub mod registry;

use thiserror::Error;

pub use command::Command;
pub use dry_run::ValidationReport;
pub use envelope::{CommandEnvelope, COMMAND_SCHEMA_VERSION};
pub use history::{AppliedTransaction, History};
pub use processor::{CommandOutcome, CommandProcessor, Notification};

/// Command-layer errors — the spec §12.11 set. Migration failures surface
/// from anim_project; everything else is carried here for uniform caller
/// handling.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CommandError {
    #[error("unsupported envelope schema version {found} (supported: {supported})")]
    UnsupportedSchema { found: u32, supported: u32 },
    #[error("revision conflict: envelope expects {expected}, document is at {actual}")]
    RevisionConflict { expected: u64, actual: u64 },
    #[error("command replay mismatch: {0}")]
    CommandReplayMismatch(String),
    #[error("empty batch rejected")]
    EmptyBatch,
    #[error("unknown command type: {0}")]
    UnknownCommand(String),
    #[error("validation: {0}")]
    Validation(String),
    #[error("revision overflow")]
    RevisionOverflow,
    #[error("undo history is empty")]
    UndoEmpty,
    #[error("redo history is empty")]
    RedoEmpty,
    #[error("persistence: {0}")]
    Persistence(String),
}

impl CommandError {
    /// Wrap a document-layer error into the matching command error.
    pub(crate) fn from_project(e: &anim_project::ProjectError) -> Self {
        CommandError::Validation(format!("document: {e}"))
    }
}

pub type CommandResult<T> = Result<T, CommandError>;
