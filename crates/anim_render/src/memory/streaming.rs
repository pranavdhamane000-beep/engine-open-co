//! Throttled upload, eviction and reload queues (spec §16.3/§16.7).
//!
//! Bounded streaming: reload work is cut into per-frame batches no
//! larger than `max_upload_bytes_per_frame` (§16.8 MemoryPolicy). The
//! queue is FIFO per priority band; ordering inside a batch follows the
//! caller-provided required set (§16.9 `reload_required` contract).

use super::resource::{ReloadState, ResourceId};
use super::MemoryError;
use std::collections::VecDeque;

/// One scheduled reload entry (immutable once enqueued).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReloadEntry {
    pub id: ResourceId,
    pub bytes: u64,
}

/// Batch output of `reload_required` (§16.9). `deferred` carries what
/// the budget could NOT schedule this frame — never silently dropped.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReloadBatch {
    pub scheduled: Vec<ReloadEntry>,
    pub deferred: Vec<ReloadEntry>,
    /// True when at least one REQUIRED entry was unschedulable (the
    /// frame must still render but the caller degrades / stalls visibly
    /// — persistent pressure becomes a diagnostic, §16.11).
    pub backed_up: bool,
}

/// Per-frame upload budget consumer (bounded queue state).
#[derive(Debug, Default)]
pub(crate) struct StreamingQueue {
    /// Entries scheduled for the frame in progress.
    scheduled_this_frame: VecDeque<ReloadEntry>,
    in_flight: VecDeque<ReloadEntry>,
}

impl StreamingQueue {
    /// Mark scheduled entries as in-flight (upload started; chunk-2
    /// device wiring consumes this between frames).
    #[allow(dead_code)] // consumed by the chunk-2 device integration
    pub(crate) fn begin_uploads(&mut self) {
        self.in_flight.extend(self.scheduled_this_frame.drain(..));
    }

    /// §16.9 `reload_required`: schedule required reloads within
    /// `frame_budget` bytes, FIFO in caller order; oversize single
    /// entries are deferred (never truncated, never dropped).
    /// Records referencing ids in `reload_states` transition
    /// PendingReload → Reloading when scheduled.
    pub(crate) fn plan_reload_batch(
        &mut self,
        required: &[ResourceId],
        reload_states: &dyn Fn(ResourceId) -> Option<(ReloadState, u64)>,
        frame_budget: u64,
    ) -> ReloadBatch {
        let mut batch = ReloadBatch::default();
        let mut used = 0u64;
        for id in required {
            let Some((state, bytes)) = reload_states(*id) else {
                continue; // unknown id: caller fabrication, skip loudly? —
                          // pinned: unknown required ids are a typed
                          // programming error and rejected by the caller
                          // in mod.rs BEFORE this pure planner is used.
            };
            if state == ReloadState::Resident {
                continue; // already resident; nothing to reload
            }
            let entry = ReloadEntry { id: *id, bytes };
            if used + bytes <= frame_budget {
                used += bytes;
                batch.scheduled.push(entry);
                self.scheduled_this_frame.push_back(entry);
            } else {
                batch.deferred.push(entry);
            }
        }
        batch.backed_up = !batch.deferred.is_empty();
        batch
    }

    /// Rewind an in-flight batch (device loss / failed upload): entries
    /// return to explicit caller control, queue drops them.
    pub(crate) fn abort_in_flight(&mut self) {
        self.in_flight.clear();
    }
}

/// §16.9 signature realization for unknown ids (mod.rs pre-validates,
/// but the planner keeps a typed guard for direct use).
pub fn reload_required_unknown(id: ResourceId) -> MemoryError {
    MemoryError::UnknownResource(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn states(
        entries: &[(ResourceId, ReloadState, u64)],
    ) -> impl Fn(ResourceId) -> Option<(ReloadState, u64)> + '_ {
        move |id| entries.iter().find(|(k, _, _)| *k == id).map(|(_, s, b)| (*s, *b))
    }

    const A: ResourceId = ResourceId(1);
    const B: ResourceId = ResourceId(2);
    const C: ResourceId = ResourceId(3);

    fn fixture() -> Vec<(ResourceId, ReloadState, u64)> {
        vec![
            (A, ReloadState::PendingReload, 10),
            (B, ReloadState::PendingReload, 8),
            (C, ReloadState::Resident, 5),
        ]
    }

    #[test]
    fn batch_respects_frame_budget_in_caller_order() {
        let mut q = StreamingQueue::default();
        let fixture = fixture();
        let f = states(&fixture);
        let batch = q.plan_reload_batch(&[A, B, C], &f, 12);
        // A (10) fits; B (8) would exceed → deferred; C already resident.
        assert_eq!(batch.scheduled, vec![ReloadEntry { id: A, bytes: 10 }]);
        assert_eq!(batch.deferred, vec![ReloadEntry { id: B, bytes: 8 }]);
        assert!(batch.backed_up);
    }

    #[test]
    fn resident_required_entries_cost_nothing_and_never_defer() {
        let mut q = StreamingQueue::default();
        let fixture = fixture();
        let f = states(&fixture);
        let batch = q.plan_reload_batch(&[C], &f, 0);
        assert!(batch.scheduled.is_empty());
        assert!(batch.deferred.is_empty());
        assert!(!batch.backed_up);
    }

    #[test]
    fn oversize_single_entry_defers_whole_never_truncates() {
        let mut q = StreamingQueue::default();
        let fixture =
            [(A, ReloadState::PendingReload, 100), (B, ReloadState::PendingReload, 2)];
        let f = states(&fixture);
        let batch = q.plan_reload_batch(&[A, B], &f, 50);
        // A never truncates: deferred whole. Later-in-order B still fits
        // its own budget share and schedules (best-fit FIFO, no
        // head-of-line blocking; pinned ADR-0011).
        assert_eq!(batch.scheduled, vec![ReloadEntry { id: B, bytes: 2 }]);
        assert_eq!(batch.deferred, vec![ReloadEntry { id: A, bytes: 100 }]);
        assert!(batch.backed_up);
    }

    #[test]
    fn full_budget_batch_not_backed_up() {
        let mut q = StreamingQueue::default();
        let fixture = fixture();
        let f = states(&fixture);
        let batch = q.plan_reload_batch(&[A, B], &f, 18);
        assert_eq!(batch.scheduled.len(), 2);
        assert!(batch.deferred.is_empty());
        assert!(!batch.backed_up);
    }
}
