//! Shared validation WITHOUT commit (spec 12.3, 12.6).
//!
//! Dry-run runs the SAME handlers against a cloned document and reports
//! what WOULD happen. It never mutates, never records history, never
//! registers idempotency receipts — apply still repeats all checks under
//! the writer boundary (spec 12.6: "This prevents a validate-then-apply
//! race").

use crate::envelope::CommandEnvelope;
use crate::{CommandError, CommandResult};
use anim_core::Revision;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationReport {
    /// Document revision the dry-run evaluated against.
    pub start_revision: Revision,
    /// Revision the document WOULD reach (exactly one per batch).
    pub resulting_revision: Revision,
    /// Number of commands that validated and would apply.
    pub applied: usize,
    /// Per-command human diagnostics in batch order.
    pub diagnostics: Vec<String>,
}

impl ValidationReport {
    /// The shared execution used by both dry-run and apply internals.
    pub(crate) fn execute_on_clone(
        document: &anim_project::ProjectDocument,
        envelope: &CommandEnvelope,
    ) -> CommandResult<(
        anim_project::ProjectDocument,
        Vec<crate::command::Command>,
        ValidationReport,
    )> {
        envelope.check_supported_schema()?;
        if envelope.commands.is_empty() {
            return Err(CommandError::EmptyBatch);
        }
        if envelope.project_id != document.project_id {
            return Err(CommandError::Validation(format!(
                "envelope targets project {}, document is {}",
                envelope.project_id, document.project_id
            )));
        }
        if envelope.expected_scene_revision != document.revision {
            return Err(CommandError::RevisionConflict {
                expected: envelope.expected_scene_revision.as_u64(),
                actual: document.revision.as_u64(),
            });
        }
        let _ =
            document.revision.checked_add(1).map_err(|_| CommandError::RevisionOverflow)?; // capacity, checked early

        let mut clone = document.clone();
        let mut inverses = Vec::with_capacity(envelope.commands.len());
        let mut diagnostics = Vec::with_capacity(envelope.commands.len());
        for (idx, command) in envelope.commands.iter().enumerate() {
            let inverse = command.apply(&mut clone).map_err(|e| match e {
                CommandError::Validation(msg) => CommandError::Validation(format!(
                    "command #{idx} ({}): {msg}",
                    command.type_name()
                )),
                other => other,
            })?;
            diagnostics.push(format!(
                "#{} {} → inverse {}",
                idx,
                command.type_name(),
                inverse.type_name()
            ));
            inverses.push(inverse);
        }
        // full-document invariants after the WHOLE batch (cross-command
        // corruption can only be seen here)
        anim_project::schema::validate_document(&clone)
            .map_err(|e| CommandError::from_project(&e))?;

        let resulting_revision =
            document.revision.checked_add(1).map_err(|_| CommandError::RevisionOverflow)?;
        let report = ValidationReport {
            start_revision: document.revision,
            resulting_revision,
            applied: envelope.commands.len(),
            diagnostics,
        };
        Ok((clone, inverses, report))
    }
}
