//! Applied transaction storage (spec 12.3, 12.6, 12.8).
//!
//! A transaction lives in EXACTLY ONE history stack: forward commits push
//! to the undo stack (clearing redo); undo pops undo → pushes redo; redo
//! pops redo → pushes undo. History is session-local and is NOT persisted
//! as authoritative project state (spec 12.6) — after reload, undo is empty.

use crate::command::Command;
use anim_assets::ContentHash;
use anim_core::Revision;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppliedTransaction {
    pub envelope_hash: ContentHash,
    /// Forward commands in original envelope order (redo re-runs exactly these).
    pub forward: Vec<Command>,
    /// Inverse commands, same index alignment as `forward` (undo applies
    /// them in REVERSE order).
    pub inverse: Vec<Command>,
    pub resulting_revision: Revision,
}

#[derive(Debug, Clone, Default)]
pub struct History {
    undo: Vec<AppliedTransaction>,
    redo: Vec<AppliedTransaction>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    /// A new forward commit: pushed to undo, redo cleared (spec 12.6
    /// "a new forward edit clears redo").
    pub fn push_forward(&mut self, txn: AppliedTransaction) {
        self.undo.push(txn);
        self.redo.clear();
    }

    /// Push to the undo stack WITHOUT clearing redo (undo/redo moves do
    /// not count as new forward edits).
    pub fn push_undo(&mut self, txn: AppliedTransaction) {
        self.undo.push(txn);
    }

    /// Pop the most recent transaction for undoing; it MOVES stacks.
    pub fn pop_undo(&mut self) -> Option<AppliedTransaction> {
        self.undo.pop()
    }

    /// Park an undone transaction on the redo stack.
    pub fn push_redo(&mut self, txn: AppliedTransaction) {
        self.redo.push(txn);
    }

    /// Pop the most recently undone transaction for redoing; it MOVES stacks.
    pub fn pop_redo(&mut self) -> Option<AppliedTransaction> {
        self.redo.pop()
    }

    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}
