//! Submitted/completed serial tracking (spec §16.7 Fence lifecycle).
//!
//! One serial is assigned at EVERY queue submission. `last_used_serial`
//! on records stores the highest serial that referenced each resource.
//! Eviction requires `last_used_serial <= completed_serial` — in-flight
//! resources are never candidates (§16.7, §16.11).
//!
//! Device loss invalidates serial assumptions: `reset_for_device_loss`
//! zeroes everything and callers rebuild residency from CPU/disk sources
//! (§16.7, handled in `recovery.rs`).

/// Tracks submission/completion serials monotonically without wrap
/// (u64 is never exhausted in practice; spec asks for monotonicity only).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FenceTracker {
    /// Highest serial handed out by `assign_next`.
    next_serial: u64,
    /// Highest serial whose submission call has been recorded.
    submitted: u64,
    /// Highest serial whose completion callback has been recorded.
    completed: u64,
}

impl FenceTracker {
    pub fn new() -> Self {
        Self { next_serial: 0, submitted: 0, completed: 0 }
    }

    /// Serial for the NEXT queue submission (§16.7 assignment point).
    /// The caller records it on every resource used by that submission.
    pub fn assign_next(&mut self) -> u64 {
        self.next_serial += 1;
        self.next_serial
    }

    /// Submission recorded (serials arrive in submission order via the
    /// render path; out-of-order is still accepted monotonically).
    pub fn record_submitted(&mut self, serial: u64) {
        self.submitted = self.submitted.max(serial);
    }

    /// Completion recorded (callback order per §16.7; monotonic).
    pub fn record_completed(&mut self, serial: u64) {
        self.completed = self.completed.max(serial);
    }

    pub fn submitted_high(&self) -> u64 {
        self.submitted
    }

    pub fn completed_high(&self) -> u64 {
        self.completed
    }

    /// Per §16.7: evict only when `last_used_serial <= completed_serial`.
    pub fn is_settled(&self, last_used_serial: u64) -> bool {
        last_used_serial <= self.completed
    }

    /// Device loss invalidates ALL serial assumptions (§16.7 tail):
    /// counters restart at 0; residency records must be rebuilt by
    /// recovery, which re-registers usage from frame 0.
    pub fn reset_for_device_loss(&mut self) {
        *self = Self::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serials_are_monotonic_and_completion_gates_eviction() {
        let mut f = FenceTracker::new();
        assert_eq!(f.assign_next(), 1);
        let s2 = f.assign_next();
        assert_eq!(s2, 2);

        // Nothing submitted/completed yet: even serial-0 usage is settled
        // only at completed >= 0 — i.e. serial 0 placeholder always settled.
        assert!(f.is_settled(0));
        assert!(!f.is_settled(1));

        f.record_submitted(1);
        assert_eq!(f.submitted_high(), 1);

        // Completion of serial 1 settles serial-1 resources.
        f.record_completed(1);
        assert!(f.is_settled(1));
        assert!(!f.is_settled(2), "serial 2 was submitted but not completed");

        // Monotonic max: a stale callback cannot un-complete work.
        f.record_completed(1);
        f.record_submitted(2);
        assert_eq!(f.completed_high(), 1);
    }

    #[test]
    fn device_loss_resets_everything() {
        let mut f = FenceTracker::new();
        let s = f.assign_next();
        f.record_submitted(s);
        f.record_completed(s);
        assert!(f.is_settled(s));
        f.reset_for_device_loss();
        assert_eq!(f.assign_next(), 1, "serial space restarts");
        assert_eq!(f.submitted_high(), 0);
        assert_eq!(f.completed_high(), 0);
        assert!(!f.is_settled(s), "old serial 1 is now inflight by definition");
    }
}
