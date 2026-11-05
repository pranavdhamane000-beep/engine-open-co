//! Versioned envelopes and batch hashes (spec 12.3, 12.8).
//!
//! The envelope is the ONLY submission shape: identical for editor, CLI and
//! future automation (spec 12.10). Its canonical serialization (JSON, field
//! declaration order, BTreeMap-free) is hashed for command-ID idempotency:
//! a replayed command ID is honored ONLY when the complete canonical hash
//! matches (spec 12.7).

use crate::command::Command;
use crate::CommandError;
use anim_assets::ContentHash;
use anim_core::{CommandId, ProjectId, Revision};
use serde::{Deserialize, Serialize};

/// Bumped by envelope/behavior changes; `schemas/commands/` holds the
/// matching external JSON schema (documentation contract).
pub const COMMAND_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandEnvelope {
    pub schema_version: u32,
    pub project_id: ProjectId,
    pub command_id: CommandId,
    /// DOCUMENT revision the batch expects (spec §12.8 field naming kept
    /// for conformance; it guards the authoritative project revision).
    pub expected_scene_revision: Revision,
    pub transaction_label: String,
    pub commands: Vec<Command>,
}

impl CommandEnvelope {
    /// A fresh envelope for a document (schema version filled, random id).
    pub fn new(
        document: &anim_project::ProjectDocument,
        transaction_label: impl Into<String>,
        commands: Vec<Command>,
    ) -> Self {
        Self {
            schema_version: COMMAND_SCHEMA_VERSION,
            project_id: document.project_id,
            command_id: CommandId::new_v4(),
            expected_scene_revision: document.revision,
            transaction_label: transaction_label.into(),
            commands,
        }
    }

    /// Complete canonical serialization hashed (BLAKE3 via the fixed-size
    /// ContentHash type). Field order + Vec order are all that exist, so
    /// any difference in content or order changes the hash.
    pub fn canonical_hash(&self) -> Result<ContentHash, CommandError> {
        let bytes = serde_json::to_vec(self)
            .map_err(|e| CommandError::Validation(format!("envelope serialize: {e}")))?;
        let mut hasher = anim_assets::hash::StreamingHasher::new();
        for chunk in bytes.chunks(anim_assets::store::STREAM_CHUNK) {
            hasher.update(chunk);
        }
        Ok(hasher.finalize())
    }

    /// Shape checks the processor re-runs under the writer boundary (spec
    /// 12.6: "apply repeats all checks under the writer boundary").
    pub(crate) fn check_supported_schema(&self) -> Result<(), CommandError> {
        if self.schema_version != COMMAND_SCHEMA_VERSION {
            return Err(CommandError::UnsupportedSchema {
                found: self.schema_version,
                supported: COMMAND_SCHEMA_VERSION,
            });
        }
        Ok(())
    }
}
