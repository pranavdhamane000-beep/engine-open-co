//! Revision checks, atomic apply, undo and redo (spec 12.3, 12.6, 12.7).
//!
//! Acceptance order is fixed (§12.7): schema → command-ID replay → project
//! ID → expected revision → nonempty batch → revision capacity → batch
//! execution on a clone → full-document validation → ONE revision bump →
//! commit → history + idempotency receipts → notifications.
//!
//! Any command error drops the ENTIRE clone and all inverses: a batch
//! prefix never commits (spec 12.11).

use crate::command::Command;
use crate::dry_run::ValidationReport;
use crate::envelope::{CommandEnvelope, COMMAND_SCHEMA_VERSION};
use crate::history::{AppliedTransaction, History};
use crate::{CommandError, CommandResult};
use anim_assets::ContentHash;
use anim_core::{CommandId, Revision};
use anim_project::ProjectDocument;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutcome {
    /// Revision the document reached after this atomic commit.
    pub new_revision: Revision,
    pub transaction_label: String,
    /// Canonical envelope hash — caller-side correlation for idempotency.
    pub envelope_hash: ContentHash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Receipt {
    Accepted { envelope_hash: ContentHash, outcome: CommandOutcome },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notification {
    Applied { revision: Revision, label: String },
    Undone { revision: Revision, label: String },
    Redone { revision: Revision, label: String },
}

pub struct CommandProcessor {
    document: ProjectDocument,
    history: History,
    receipts: HashMap<CommandId, Receipt>,
    notifications: Vec<Notification>,
}

impl CommandProcessor {
    pub fn new(document: ProjectDocument) -> Self {
        Self {
            document,
            history: History::new(),
            receipts: HashMap::new(),
            notifications: Vec::new(),
        }
    }

    /// The authoritative document (read-only; mutations ONLY via commands).
    pub fn document(&self) -> &ProjectDocument {
        &self.document
    }

    pub fn undo_depth(&self) -> usize {
        self.history.undo_depth()
    }

    pub fn redo_depth(&self) -> usize {
        self.history.redo_depth()
    }

    /// Shared validation without commit (spec 12.6). Replays the SAME
    /// handlers on a clone; nothing is recorded anywhere.
    pub fn dry_run(&self, envelope: &CommandEnvelope) -> CommandResult<ValidationReport> {
        envelope.check_supported_schema()?;
        let (_, _, report) = ValidationReport::execute_on_clone(&self.document, envelope)?;
        Ok(report)
    }

    /// Atomic apply (spec 12.7 algorithm, in the specified check order).
    pub fn apply(&mut self, envelope: CommandEnvelope) -> CommandResult<CommandOutcome> {
        // 1. schema
        envelope.check_supported_schema()?;

        // 2. command-ID replay (BEFORE revision checks — an exact duplicate
        //    accepted during later progress still returns its ORIGINAL
        //    result; mismatched content rejects as replay mismatch).
        let hash = envelope.canonical_hash()?;
        match self.receipts.get(&envelope.command_id) {
            Some(Receipt::Accepted { outcome, envelope_hash })
                if *envelope_hash == hash =>
            {
                return Ok(outcome.clone())
            }
            Some(Receipt::Accepted { envelope_hash, .. }) => {
                return Err(CommandError::CommandReplayMismatch(format!(
                    "command id {} was accepted with a different envelope (stored hash {}, now {})",
                    envelope.command_id,
                    envelope_hash.full_hex(),
                    hash.full_hex()
                )));
            }
            None => {}
        }

        // 3–7: shared execution on a clone (project ID, expected revision,
        // nonempty batch, capacity, per-command validation, doc invariants).
        let (clone, inverses, _) =
            ValidationReport::execute_on_clone(&self.document, &envelope)?;

        let candidate = self
            .document
            .revision
            .checked_add(1)
            .map_err(|_| CommandError::RevisionOverflow)?;

        let outcome = CommandOutcome {
            new_revision: candidate,
            transaction_label: envelope.transaction_label.clone(),
            envelope_hash: hash,
        };

        // 8. commit: swap the clone, bump revision ONCE per batch
        self.document = clone;
        self.document.revision = candidate;

        // history: push to undo, clear redo (new forward edit)
        self.history.push_forward(AppliedTransaction {
            envelope_hash: hash,
            forward: envelope.commands.clone(),
            inverse: inverses,
            resulting_revision: candidate,
        });

        // idempotency receipt (accepted envelope)
        self.receipts.insert(
            envelope.command_id,
            Receipt::Accepted { envelope_hash: hash, outcome: outcome.clone() },
        );

        // notifications AFTER commit (spec 12.6)
        self.notifications.push(Notification::Applied {
            revision: candidate,
            label: envelope.transaction_label,
        });

        Ok(outcome)
    }

    /// Undo the most recent transaction: inverses in REVERSE order on a
    /// clone, ONE new revision, transaction MOVES to the redo stack.
    pub fn undo(&mut self) -> CommandResult<CommandOutcome> {
        let txn = self.history.pop_undo().ok_or(CommandError::UndoEmpty)?;
        match self.commit_direction(&txn, Direction::Inverse) {
            Ok(outcome) => {
                self.history.push_redo(txn);
                self.notifications.push(Notification::Undone {
                    revision: outcome.new_revision,
                    label: outcome.transaction_label.clone(),
                });
                Ok(outcome)
            }
            // failure restores the transaction to the UNDO stack, document
            // untouched (defensive: inverses were validated when created)
            Err(e) => {
                self.history.push_undo(txn);
                Err(e)
            }
        }
    }

    /// Redo: reapply the forward commands of the most recently undone
    /// transaction, in ORIGINAL forward order.
    pub fn redo(&mut self) -> CommandResult<CommandOutcome> {
        let txn = self.history.pop_redo().ok_or(CommandError::RedoEmpty)?;
        match self.commit_direction(&txn, Direction::Forward) {
            Ok(outcome) => {
                // transaction returns to the UNDO stack (the remaining redo
                // stack is untouched only NEW forward edits clear redo)
                self.history.push_undo(txn);
                self.notifications.push(Notification::Redone {
                    revision: outcome.new_revision,
                    label: outcome.transaction_label.clone(),
                });
                Ok(outcome)
            }
            Err(e) => {
                self.history.push_redo(txn);
                Err(e)
            }
        }
    }

    /// Drain published notifications (after-commit events, spec 12.6).
    pub fn take_notifications(&mut self) -> Vec<Notification> {
        std::mem::take(&mut self.notifications)
    }

    /// Run a direction's command list on a clone; commit on success.
    fn commit_direction(
        &mut self,
        txn: &AppliedTransaction,
        direction: Direction,
    ) -> CommandResult<CommandOutcome> {
        if txn.inverse.len() != txn.forward.len() {
            return Err(CommandError::Validation(
                "transaction inverse/forward length mismatch".into(),
            ));
        }
        let commands: Vec<Command> = match direction {
            Direction::Inverse => txn.inverse.iter().rev().cloned().collect(),
            Direction::Forward => txn.forward.clone(),
        };
        if commands.is_empty() {
            return Err(CommandError::Validation("empty transaction executed".into()));
        }
        let mut clone = self.document.clone();
        for command in &commands {
            command.apply(&mut clone)?;
        }
        // full invariants after the whole direction (cross-command corruption)
        anim_project::schema::validate_document(&clone)
            .map_err(|e| CommandError::from_project(&e))?;
        let candidate = self
            .document
            .revision
            .checked_add(1)
            .map_err(|_| CommandError::RevisionOverflow)?;

        let short = txn.envelope_hash.full_hex().chars().take(8).collect::<String>();
        let label = format!("undo/redo of {short}");
        let outcome = CommandOutcome {
            new_revision: candidate,
            transaction_label: label,
            envelope_hash: txn.envelope_hash,
        };
        self.document = clone;
        self.document.revision = candidate;
        Ok(outcome)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Forward,
    Inverse,
}

/// Schema version constant re-exported for callers building envelopes.
pub const SUPPORTED_SCHEMA_VERSION: u32 = COMMAND_SCHEMA_VERSION;
