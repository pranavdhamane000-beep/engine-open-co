//! Allocation preflight, reservations and rollback (spec §16.7).
//!
//! Flow per §16.7 "Allocation preflight": obtain a fresh snapshot within
//! policy → estimate bytes → reserve. If the projection crosses the soft
//! target (the hard trigger at sampling level), an eviction plan is
//! applied FIRST. `AllocationPermit` is the ownership token: dropping it
//! releases the reservation; only an explicit commit (after the ACTUAL
//! wgpu creation succeeded) consumes it. Actual allocation errors
//! bypass all sampling delays (§16.6): rollback is immediate and the
//! orchestrator runs ONE bounded additional eviction before a single
//! retry, only when policy allows (pinned: `MAX_ALLOCATION_RETRIES`).
//!
//! Projection accounting is conservative: outstanding UNCOMMITTED
//! reservations count against the target so the same bytes are never
//! double-promised to two callers. The ledger is shared between the
//! manager and its live permits through an `Rc<RefCell>` (single
//! render-thread ownership; §16.4 "standard synchronization" note: WP07
//! MVP runs the memory manager on the encoder thread only).

use super::eviction::{BudgetSnapshot, EvictionPlan};
use super::resource::{ResourceId, ResourcePriority};
use super::MemoryError;
use std::cell::RefCell;
use std::rc::Rc;

/// Bounded retry budget (§16.7 "retry only if the policy allows" —
/// pinned to ONE round of bounded extra eviction/degradation).
/// Consumed by the device allocation loop (chunk 2).
#[allow(dead_code)]
pub(crate) const MAX_ALLOCATION_RETRIES: u8 = 1;

/// Ledger key for one reservation (opaque; never enters public records
/// so an aborted preflight cannot release the wrong entry).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReservationId(pub(crate) u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Reservation {
    pub(crate) bytes: u64,
    pub(crate) priority: ResourcePriority,
}

/// Reservation ledger (id-addressed take → drop-safe).
#[derive(Debug, Default)]
pub struct ReservationLedger {
    next: u64,
    open: Vec<(ReservationId, Reservation)>,
}

impl ReservationLedger {
    pub(crate) fn reserve(
        &mut self,
        bytes: u64,
        priority: ResourcePriority,
    ) -> ReservationId {
        self.next += 1;
        let id = ReservationId(self.next);
        self.open.push((id, Reservation { bytes, priority }));
        id
    }

    pub(crate) fn total_open_bytes(&self) -> u64 {
        self.open.iter().map(|(_, r)| r.bytes).sum()
    }

    #[allow(dead_code)] // tests use it; keep for diagnostics parity
    pub(crate) fn open_count(&self) -> usize {
        self.open.len()
    }

    /// Remove one entry (commit-consume OR release-on-drop); the
    /// reservation if it was still open, `None` otherwise (consumed).
    pub(crate) fn take(&mut self, id: ReservationId) -> Option<Reservation> {
        let pos = self.open.iter().position(|(k, _)| *k == id)?;
        Some(self.open.remove(pos).1)
    }

    /// Device loss invalidates commitments' meaning: uncommitted
    /// reservations are cleared (their callers re-enter via the rebuilt
    /// render path, §16.7 device-loss reconstruction).
    pub(crate) fn reset_for_device_loss(&mut self) {
        self.open.clear();
    }
}

/// Shared ledger handle type (manager ⟷ live permits).
pub type SharedLedger = Rc<RefCell<ReservationLedger>>;

/// Public permit (§16.8): releases its reservation on drop unless
/// committed; commit consumes the ledger entry exactly once.
#[derive(Debug)]
pub struct AllocationPermit {
    ledger: SharedLedger,
    id: ReservationId,
    bytes: u64,
    priority: ResourcePriority,
    committed: bool,
}

impl AllocationPermit {
    /// Manager-side constructor (shared-ledger linkage).
    pub(crate) fn shared(
        ledger: SharedLedger,
        id: ReservationId,
        bytes: u64,
        priority: ResourcePriority,
    ) -> Self {
        Self { ledger, id, bytes, priority, committed: false }
    }

    /// Bytes the caller MUST underwrite with the real device allocation.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn priority(&self) -> ResourcePriority {
        self.priority
    }

    /// Commit after the real resource creation SUCCEEDED. The ledger
    /// entry is consumed here; `register_resource` follows and returns
    /// the authoritative `ResourceId` (the placeholder keeps the §16.9
    /// `commit() -> ResourceId` signature shape).
    pub fn commit(mut self) -> ResourceId {
        self.committed = true;
        let _ = self.ledger.borrow_mut().take(self.id);
        ResourceId::PLACEHOLDER
    }
}

impl Drop for AllocationPermit {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.ledger.borrow_mut().take(self.id);
        }
    }
}

/// OOM byte context (§16.11 "OOM returns requested bytes, budget, usage"
/// after bounded recovery — richer than the minimum).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorContext {
    pub requested: u64,
    /// The soft target the projection was checked against.
    pub budget: u64,
    pub effective_usage: u64,
    pub reservation_total: u64,
    /// How many bytes over the target the best projection was.
    pub shortfall: u64,
}

impl ErrorContext {
    pub(crate) fn into_memory_error(self, detail: &'static str) -> MemoryError {
        MemoryError::OutOfMemory {
            requested: self.requested,
            budget: self.budget,
            effective_usage: self.effective_usage,
            reservation_total: self.reservation_total,
            shortfall: self.shortfall,
            detail: detail.into(),
        }
    }
}

/// Pure preflight outcome (§16.7 decision order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PreflightOutcome {
    /// Projection stays under the soft target: reserve directly.
    Grant,
    /// Reserve AFTER applying this satisfied plan; the caller MUST run
    /// the releases before the real device allocation.
    GrantAfterEviction { plan: EvictionPlan },
    /// Even the plan cannot fit within policy — bounded, actionable
    /// refusal with full byte context.
    Refuse { refused: ErrorContext },
}

/// Pure preflight computation (§16.7). No I/O, no wall clock — fully
/// unit-testable against synthetic snapshots (§16.12).
///
/// `snapshot` = current budget view; `reservation_open` = open
/// uncommitted bytes; `plan` = eviction plan for the REQUIRED set (may
/// be empty/unsatisfied).
pub(crate) fn preflight(
    snapshot: &BudgetSnapshot,
    reservation_open: u64,
    requested: u64,
    soft_target: u64,
    plan: &EvictionPlan,
) -> PreflightOutcome {
    let effective = snapshot.effective_usage;
    let released: u64 = plan.releases.iter().map(|c| c.bytes).sum();

    let projected_now = effective + reservation_open + requested;
    let projected_after_eviction = effective
        .saturating_sub(released.min(effective))
        .saturating_add(reservation_open + requested);

    if projected_now <= soft_target {
        return PreflightOutcome::Grant;
    }
    if plan.satisfied && projected_after_eviction <= soft_target {
        return PreflightOutcome::GrantAfterEviction { plan: plan.clone() };
    }
    // Refusal projection: the best the policy could do.
    let best_projected =
        if plan.satisfied { projected_after_eviction } else { projected_now };
    PreflightOutcome::Refuse {
        refused: ErrorContext {
            requested,
            budget: soft_target,
            effective_usage: effective,
            reservation_total: reservation_open,
            shortfall: best_projected.saturating_sub(soft_target),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::super::eviction::{compute_targets, EvictCandidate, EvictionReason};
    use super::*;

    fn shared() -> SharedLedger {
        Rc::new(RefCell::new(ReservationLedger::default()))
    }

    fn snapshot(effective: u64, budget: u64) -> BudgetSnapshot {
        BudgetSnapshot {
            budget,
            current_usage: effective,
            current_reservation: 0,
            available_for_reservation: budget.saturating_sub(effective),
            accounted_usage: effective,
            effective_usage: effective,
        }
    }

    fn empty_plan() -> EvictionPlan {
        EvictionPlan::default()
    }

    fn plan_releasing(bytes: u64, satisfied: bool) -> EvictionPlan {
        EvictionPlan {
            releases: vec![EvictCandidate {
                id: ResourceId(1),
                bytes,
                reason: EvictionReason::LowPriority,
            }],
            projected_usage: 0,
            satisfied,
        }
    }

    fn kind(o: &PreflightOutcome) -> &'static str {
        match o {
            PreflightOutcome::Grant => "grant",
            PreflightOutcome::GrantAfterEviction { .. } => "evict-then-grant",
            PreflightOutcome::Refuse { .. } => "refuse",
        }
    }

    #[test]
    fn grants_under_target_counting_open_reservations() {
        // Synthetic budget 1000, fraction 0.8, no reserve → target 800.
        let t = compute_targets(1000, 0.80, 0, 0);
        assert_eq!(t.soft_target, 800);
        // effective 600 + open 100 + request 100 == 800 ≤ 800: grant.
        let o = preflight(&snapshot(600, 1000), 100, 100, t.soft_target, &empty_plan());
        assert_eq!(kind(&o), "grant");

        // One byte more crosses the trigger with an EMPTY plan → refuse
        // with the exact byte context (§16.11 actionable shape).
        match preflight(&snapshot(600, 1000), 100, 101, t.soft_target, &empty_plan()) {
            PreflightOutcome::Refuse { refused } => {
                assert_eq!(refused.requested, 101);
                assert_eq!(refused.reservation_total, 100);
                assert_eq!(refused.shortfall, 1);
            }
            other => panic!("expected refusal, got {other:?}"),
        }
    }

    #[test]
    fn grants_after_satisfied_eviction_when_projection_fits() {
        let t = compute_targets(1000, 0.80, 0, 0);
        // effective 900 + request 100 == 1000 > 800; a satisfied plan
        // releasing 250 → 900−250+100 == 750 ≤ 800: evict-then-grant.
        let plan = plan_releasing(250, true);
        match preflight(&snapshot(900, 1000), 0, 100, t.soft_target, &plan) {
            PreflightOutcome::GrantAfterEviction { plan } => {
                assert_eq!(plan.releases[0].bytes, 250);
            }
            other => panic!("expected evict-then-grant, got {other:?}"),
        }
    }

    #[test]
    fn refuses_when_plan_satisfied_but_projection_still_over() {
        let t = compute_targets(1000, 0.80, 0, 0);
        // Release only 150 → 900−150+100 == 850 > 800: refuse even
        // though the plan was satisfied for its own watermark.
        let plan = plan_releasing(150, true);
        match preflight(&snapshot(900, 1000), 0, 100, t.soft_target, &plan) {
            PreflightOutcome::Refuse { refused } => {
                assert_eq!(refused.shortfall, 50);
            }
            other => panic!("expected refusal, got {other:?}"),
        }
    }

    #[test]
    fn releases_never_eat_reservations_in_projection() {
        let t = compute_targets(1000, 0.80, 0, 0);
        // Absurd release size saturates at effective usage: 0 + open
        // 800 + request 100 == 900 > 800 → refuse. Reservation bytes
        // are NEVER released by eviction math.
        let plan = plan_releasing(10_000, true);
        match preflight(&snapshot(900, 1000), 800, 100, t.soft_target, &plan) {
            PreflightOutcome::Refuse { refused } => {
                assert_eq!(refused.reservation_total, 800);
                assert_eq!(refused.shortfall, 100);
            }
            other => panic!("expected refusal, got {other:?}"),
        }
    }

    #[test]
    fn permit_releases_reservation_on_drop_once() {
        let ledger = shared();
        let id = ledger.borrow_mut().reserve(512, ResourcePriority::Cache);
        assert_eq!(ledger.borrow().total_open_bytes(), 512);

        let permit = AllocationPermit {
            ledger: Rc::clone(&ledger),
            id,
            bytes: 512,
            priority: ResourcePriority::Cache,
            committed: false,
        };
        assert_eq!(permit.bytes(), 512);
        drop(permit);
        assert_eq!(ledger.borrow().total_open_bytes(), 0);
        assert_eq!(ledger.borrow().open_count(), 0);
    }

    #[test]
    fn commit_consumes_ledger_and_drop_becomes_noop() {
        let ledger = shared();
        let id = ledger.borrow_mut().reserve(512, ResourcePriority::Cache);
        let permit = AllocationPermit {
            ledger: Rc::clone(&ledger),
            id,
            bytes: 512,
            priority: ResourcePriority::Cache,
            committed: false,
        };
        let committed = permit.commit();
        assert_eq!(committed, ResourceId::PLACEHOLDER);
        assert_eq!(ledger.borrow().open_count(), 0, "commit consumes exactly once");

        // A committed permit's drop consumes nothing further: a fresh
        // reservation stays open while the (committed) permit dies.
        let permit2 = AllocationPermit {
            ledger: Rc::clone(&ledger),
            id,
            bytes: 512,
            priority: ResourcePriority::Cache,
            committed: true, // simulate already-consumed state
        };
        let _ = ledger.borrow_mut().reserve(9, ResourcePriority::Cache);
        drop(permit2); // committed → no take()
        assert_eq!(ledger.borrow().open_count(), 1, "committed drop is a no-op");
        assert_eq!(ledger.borrow().total_open_bytes(), 9);
    }

    #[test]
    fn ledger_device_loss_clears_all_open_reservations() {
        let mut ledger = ReservationLedger::default();
        let _a = ledger.reserve(10, ResourcePriority::Cache);
        let _b = ledger.reserve(20, ResourcePriority::Critical);
        assert_eq!(ledger.total_open_bytes(), 30);
        ledger.reset_for_device_loss();
        assert_eq!(ledger.open_count(), 0);
        assert_eq!(ledger.total_open_bytes(), 0);
    }

    #[test]
    fn error_context_becomes_typed_memory_error() {
        let ctx = ErrorContext {
            requested: 10,
            budget: 8,
            effective_usage: 0,
            reservation_total: 0,
            shortfall: 2,
        };
        match ctx.into_memory_error("test") {
            MemoryError::OutOfMemory { requested, budget, shortfall, detail, .. } => {
                assert_eq!((requested, budget, shortfall), (10, 8, 2));
                assert!(detail.contains("test"));
            }
            other => panic!("wrong variant {other:?}"),
        }
    }
}
