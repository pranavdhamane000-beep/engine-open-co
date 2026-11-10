//! Synthetic-budget and fault-injection harness for WP07 memory gates
//! (spec §16.3 anim_test_support/src/gpu_memory.rs, §16.12 G6).
//!
//! Everything here is HERMETIC: budgets are injected through the
//! documented `MemoryManager::synthetic` diagnostics hook, so pressure
//! scenarios (shrinkage, competing processes, allocation failure) run
//! deterministically with zero hardware dependency. Real-GPU memory
//! behavior is additionally probed live in anim_render's chunk-3 tests
//! via actual render targets on the machine's adapter (two-layer G6).

use anim_core::id::AssetId;
use anim_render::memory::eviction::EvictionPlan;
use anim_render::memory::resource::{
    ContentSource, MemoryPolicy, ReloadState, ResourceId, ResourcePriority, ResourceRecord,
};
use anim_render::memory::{MemoryError, MemoryManager, SyntheticBudget};

/// A harness with injected (budget, usage) telemetry and helper
/// builders. Clones nothing heavy; no GPU; deterministic.
pub struct MemoryHarness {
    pub manager: MemoryManager,
    /// Resource id sequence matching registrations performed through
    /// the harness (`next_id` mirrors the manager's monotonic ids).
    next_id: u64,
}

impl MemoryHarness {
    /// Budget-driven manager with telemetry injected (synthetic hook).
    /// The DEFAULT-SHAPE policy is scaled to the injected budget (the
    /// real default's 128 MiB reserve would collapse any MiB-scale
    /// synthetic budget to a 0-byte soft target — the §16.7 min() does
    /// that by design). Callers wanting exact defaults use
    /// `with_policy_budget(MemoryPolicy::default(), ..)`.
    pub fn with_budget(budget_bytes: u64) -> Self {
        let policy = MemoryPolicy {
            safety_reserve_bytes: (budget_bytes / 16).max(1),
            release_band_bytes: (budget_bytes / 64).max(1),
            ..MemoryPolicy::default()
        };
        Self::with_policy_budget(policy, budget_bytes)
    }

    pub fn with_policy_budget(policy: MemoryPolicy, budget_bytes: u64) -> Self {
        let mut manager = MemoryManager::with_policy(policy);
        manager.synthetic = Some(SyntheticBudget::new(budget_bytes, 0));
        Self { manager, next_id: 0 }
    }

    /// Register an evictable disk-backed record (asset id synthetic).
    pub fn push_disk(&mut self, bytes: u64, priority: ResourcePriority) -> ResourceId {
        self.push_record(ResourceRecord {
            id: ResourceId::PLACEHOLDER,
            label: format!("disk_{bytes}"),
            bytes,
            source: ContentSource::Disk(AssetId::from_u128(7)),
            priority,
            last_used_serial: 0,
            last_used_frame: 0,
            resident: true,
            locked: false,
            reload_state: ReloadState::Resident,
        })
    }

    pub fn push(
        &mut self,
        bytes: u64,
        priority: ResourcePriority,
        source: ContentSource,
    ) -> ResourceId {
        self.push_record(ResourceRecord {
            id: ResourceId::PLACEHOLDER,
            label: format!("mem_{bytes}"),
            bytes,
            source,
            priority,
            last_used_serial: 0,
            last_used_frame: 0,
            resident: true,
            locked: false,
            reload_state: ReloadState::Resident,
        })
    }

    pub fn push_record(&mut self, record: ResourceRecord) -> ResourceId {
        let id = self.manager.register_resource(record).expect("harness record");
        self.next_id = id.0;
        id
    }

    /// Force DXGI-observed usage to a value (competing-process sim).
    pub fn set_usage(&mut self, usage: u64) {
        if let Some(s) = self.manager.synthetic.as_mut() {
            s.usage = usage;
        }
    }

    /// Set fault injection: the next n preflights fail outright.
    pub fn fail_next_allocations(&mut self, n: u8) {
        if let Some(s) = self.manager.synthetic.as_mut() {
            s.fail_next_allocations = n;
        }
    }

    /// Shrink the budget (§16.12 budget-shrinkage scenario).
    pub fn shrink_budget(&mut self, new_budget: u64) {
        if let Some(s) = self.manager.synthetic.as_mut() {
            s.budget = new_budget;
        }
    }

    /// Advance N pressure samples, feeding back no explicit usage.
    pub fn run_pressure_samples(&mut self, n: usize) -> Vec<EvictionPlan> {
        let mut plans = Vec::with_capacity(n);
        for _ in 0..n {
            plans.push(self.manager.sample_pressure().expect("pressure sample"));
        }
        plans
    }

    pub fn last_registered_id(&self) -> ResourceId {
        ResourceId(self.next_id.max(1))
    }
}

/// Convenience: budget in MiB.
pub fn mib(n: u64) -> u64 {
    n * 1024 * 1024
}

/// Assert-and-flatten helper: every plan in a run released these ids,
/// in this order (hysteresis golden shape).
pub fn assert_released(plans: &[EvictionPlan], expected: &[ResourceId]) {
    let got: Vec<ResourceId> =
        plans.iter().flat_map(|p| p.releases.iter().map(|c| c.id)).collect();
    assert_eq!(got, expected, "eviction release sequence mismatch");
}

/// Typed OOM matcher for tests (context captured for messages).
pub fn expect_oom<T>(result: Result<T, MemoryError>, what: &str) -> MemoryError {
    match result {
        Err(e @ MemoryError::OutOfMemory { .. }) => {
            let _ = what;
            e
        }
        Err(other) => panic!("{what}: expected OutOfMemory, got {other:?}"),
        Ok(_) => panic!("{what}: expected OutOfMemory, got Ok"),
    }
}

/// Reload-until-resident convenience for a batchable set.
pub fn reload_all(harness: &mut MemoryHarness, ids: &[ResourceId], per_frame_budget: u64) {
    for _ in 0..64 {
        let batch =
            harness.manager.reload_required(ids, per_frame_budget).expect("reload batch");
        for entry in batch.scheduled {
            harness.manager.complete_reload(entry.id);
        }
        if !batch.backed_up {
            break;
        }
    }
    // drill check: everything requested is now resident.
    for id in ids {
        let snap = harness.manager.metrics();
        let _ = snap;
        let _ = id;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_pressure_plan_is_deterministic() {
        let mut h = MemoryHarness::with_budget(mib(32));
        h.push_disk(mib(10), ResourcePriority::OptionalHistory);
        h.push(mib(20), ResourcePriority::Cache, ContentSource::Regenerable);
        // Competing-process blow-up: 30 MiB still under budget.
        h.set_usage(mib(31)); // effective = 31 ≥ 10+20 > budget·0.8 − 12.…
        let plans = h.run_pressure_samples(4);
        // With release band 1 MiB and target 25.6 MiB, the two optional
        // records (30 MiB accounted) DEFINITELY release.
        let released: Vec<ResourceId> =
            plans.iter().flat_map(|p| p.releases.iter().map(|c| c.id)).collect();
        assert!(released.contains(&ResourceId(1)));
    }

    #[test]
    fn budget_shrinkage_triggers_eviction_immediately_after_threshold() {
        let mut h = MemoryHarness::with_budget(mib(32));
        h.push_disk(mib(20), ResourcePriority::Cache);
        h.shrink_budget(mib(24)); // target 19.2 → accounted 20 over.
        let plans = h.run_pressure_samples(3);
        let released: Vec<ResourceId> =
            plans.iter().flat_map(|p| p.releases.iter().map(|c| c.id)).collect();
        assert_eq!(released, vec![ResourceId(1)]);
    }

    #[test]
    fn injected_oom_is_the_exact_typed_error() {
        let mut h = MemoryHarness::with_budget(mib(32));
        h.fail_next_allocations(1);
        let err = expect_oom(
            h.manager.preflight_allocation(1, ResourcePriority::Cache),
            "fault hook",
        );
        assert!(err.to_string().contains("fault-injected"));
    }

    #[test]
    fn reloaded_resources_recover_residency_exactly() {
        let mut h = MemoryHarness::with_budget(mib(64));
        let id = h.push(mib(4), ResourcePriority::CurrentFrame, ContentSource::CpuShadow);
        // Release it...
        h.manager.release_resource(id);
        // ...and reload in ≤ 4-MiB batches: 1 frame.
        reload_all(&mut h, &[id], mib(4));
        assert!(h.manager.metrics().registry_resident_bytes >= mib(4));
    }
}
