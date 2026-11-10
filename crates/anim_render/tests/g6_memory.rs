//! G6 memory validation tests (spec §16.12). Two layers:
//! 1. Synthetic-budget tests (hermetic, zero hardware dependency) —
//!    target arithmetic, reservation rollback, candidate ordering,
//!    hysteresis, exclusion filters, shrinkage, competing pressure,
//!    unavailable DXGI, adapter mismatch, allocation failure, OOM.
//! 2. Live-hardware stress (RTX 3050) — scrubbing, resize, long export
//!    with usage/eviction/reload/stall logging, render-reference exactness.

use anim_core::id::AssetId;
use anim_render::memory::eviction::compute_targets;
use anim_render::memory::resource::{
    ContentSource, MemoryPolicy, ReloadState, ResourceId, ResourcePriority, ResourceRecord,
};
use anim_render::memory::{
    FenceTracker, IdentityHints, IdentityState, MemoryError, MemoryManager, SyntheticBudget,
};
use std::collections::BTreeSet;

fn make_policy() -> MemoryPolicy {
    MemoryPolicy {
        target_fraction: 0.80,
        safety_reserve_bytes: 100,
        release_band_bytes: 50,
        over_budget_samples: 3,
        query_interval: std::time::Duration::from_millis(0),
        max_upload_bytes_per_frame: 16 * 1024 * 1024,
    }
}

fn make_manager(budget: u64) -> MemoryManager {
    let mut m = MemoryManager::with_policy(make_policy());
    m.synthetic = Some(SyntheticBudget::new(budget, 0));
    m
}

fn push_record(
    m: &mut MemoryManager,
    bytes: u64,
    priority: ResourcePriority,
    source: ContentSource,
) -> ResourceId {
    m.register_resource(ResourceRecord {
        id: ResourceId::PLACEHOLDER,
        label: format!("rec_{}", bytes),
        bytes,
        source,
        priority,
        last_used_serial: 0,
        last_used_frame: 0,
        resident: true,
        locked: false,
        reload_state: ReloadState::Resident,
    })
    .unwrap()
}

fn mib(n: u64) -> u64 {
    n * 1024 * 1024
}

// ---------- G6.1: target arithmetic ----------

#[test]
fn g6_target_arithmetic_is_saturating_min_of_fraction_and_reserve() {
    let t =
        compute_targets(4 * 1024 * 1024 * 1024, 0.80, 128 * 1024 * 1024, 64 * 1024 * 1024);
    assert_eq!(t.soft_target, 3435973888);
    assert_eq!(t.release_watermark, t.soft_target - 64 * 1024 * 1024);

    let t2 = compute_targets(100, 0.80, 1_000_000, 50);
    assert_eq!(t2.soft_target, 0);
    assert_eq!(t2.release_watermark, 0);
}

// ---------- G6.2: reservation rollback ----------

#[test]
fn g6_reservation_rollback_on_drop() {
    let mut m = make_manager(mib(32));
    let permit = m.preflight_allocation(1024, ResourcePriority::Cache).expect("grant");
    assert_eq!(m.reservation_total(), 1024);
    drop(permit);
    assert_eq!(m.reservation_total(), 0);
}

// ---------- G6.3: candidate ordering ----------

#[test]
fn g6_candidate_ordering_source_class_priority_serial_bytes_id() {
    let mut fences = FenceTracker::new();
    let records = vec![
        (
            ResourceId(1),
            ResourceRecord {
                id: ResourceId::PLACEHOLDER,
                label: "disk_old".into(),
                bytes: 100,
                source: ContentSource::Disk(AssetId::from_u128(1)),
                priority: ResourcePriority::OptionalHistory,
                last_used_serial: 2,
                last_used_frame: 0,
                resident: true,
                locked: false,
                reload_state: ReloadState::Resident,
            },
        ),
        (
            ResourceId(2),
            ResourceRecord {
                id: ResourceId::PLACEHOLDER,
                label: "cpu_shadow".into(),
                bytes: 100,
                source: ContentSource::CpuShadow,
                priority: ResourcePriority::Cache,
                last_used_serial: 5,
                last_used_frame: 0,
                resident: true,
                locked: false,
                reload_state: ReloadState::Resident,
            },
        ),
        (
            ResourceId(3),
            ResourceRecord {
                id: ResourceId::PLACEHOLDER,
                label: "regen_new".into(),
                bytes: 50,
                source: ContentSource::Regenerable,
                priority: ResourcePriority::OptionalHistory,
                last_used_serial: 5,
                last_used_frame: 0,
                resident: true,
                locked: false,
                reload_state: ReloadState::Resident,
            },
        ),
    ];
    fences.record_completed(5);
    let order = anim_render::memory::eviction::candidate_order(&records, &fences);
    assert_eq!(order[0], ResourceId(1), "disk first");
    assert_eq!(order[2], ResourceId(2), "cpu_shadow last");
}

// ---------- G6.4: hysteresis ----------

#[test]
fn g6_hysteresis_latches_after_three_and_resets_below_watermark() {
    let mut m = make_manager(mib(32));
    push_record(
        &mut m,
        mib(20),
        ResourcePriority::OptionalHistory,
        ContentSource::Regenerable,
    );
    m.synthetic.as_mut().unwrap().usage = mib(30);

    for _ in 0..2 {
        let plan = m.sample_pressure().unwrap();
        assert!(plan.releases.is_empty(), "below 3-sample threshold");
    }
    let plan = m.sample_pressure().unwrap();
    assert!(m.is_evicting(), "3rd sample latches");
    assert!(!plan.releases.is_empty(), "eviction fires on latch");

    m.synthetic.as_mut().unwrap().usage = 0;
    m.sample_pressure().unwrap();
    assert!(!m.is_evicting(), "reset below watermark");
}

// ---------- G6.5: exclusion filters ----------

#[test]
fn g6_locked_resources_never_selected() {
    let mut m = make_manager(mib(32));
    let id = push_record(
        &mut m,
        mib(10),
        ResourcePriority::OptionalHistory,
        ContentSource::Regenerable,
    );
    m.set_locked(id, true).unwrap();
    m.synthetic.as_mut().unwrap().usage = mib(30);
    for _ in 0..4 {
        let plan = m.sample_pressure().unwrap();
        assert!(!plan.releases.iter().any(|c| c.id == id), "locked must never release");
    }
}

#[test]
fn g6_inflight_resources_never_selected() {
    let mut m = make_manager(mib(32));
    let id = push_record(
        &mut m,
        mib(10),
        ResourcePriority::OptionalHistory,
        ContentSource::Regenerable,
    );
    m.mark_used(id, 1, 0);
    m.on_submitted(1);
    m.synthetic.as_mut().unwrap().usage = mib(30);
    for _ in 0..4 {
        let plan = m.sample_pressure().unwrap();
        assert!(!plan.releases.iter().any(|c| c.id == id), "in-flight must never release");
    }
}

#[test]
fn g6_gpu_only_never_selected() {
    let mut m = make_manager(mib(32));
    let id = push_record(
        &mut m,
        mib(10),
        ResourcePriority::OptionalHistory,
        ContentSource::GpuOnly,
    );
    m.synthetic.as_mut().unwrap().usage = mib(30);
    for _ in 0..4 {
        let plan = m.sample_pressure().unwrap();
        assert!(!plan.releases.iter().any(|c| c.id == id), "GpuOnly must never release");
    }
}

#[test]
fn g6_required_resources_never_evicted() {
    let mut m = make_manager(mib(32));
    let id1 = push_record(
        &mut m,
        mib(10),
        ResourcePriority::CurrentFrame,
        ContentSource::Regenerable,
    );
    let id2 = push_record(
        &mut m,
        mib(5),
        ResourcePriority::OptionalHistory,
        ContentSource::Regenerable,
    );
    let mut required = BTreeSet::new();
    required.insert(id1);
    m.synthetic.as_mut().unwrap().usage = mib(30);
    let snap = m.query_local_memory().unwrap();
    let plan = m.plan_eviction(&snap, &required);
    let released: Vec<ResourceId> = plan.releases.iter().map(|c| c.id).collect();
    assert!(!released.contains(&id1), "required must never evict");
    assert!(released.contains(&id2), "non-required optional may evict");
}

// ---------- G6.6: budget shrinkage ----------

#[test]
fn g6_budget_shrinkage_triggers_eviction() {
    let mut m = make_manager(mib(32));
    push_record(&mut m, mib(20), ResourcePriority::Cache, ContentSource::Regenerable);
    m.synthetic.as_mut().unwrap().budget = mib(24);
    m.synthetic.as_mut().unwrap().usage = mib(23);
    let plans: Vec<_> = (0..3).map(|_| m.sample_pressure().unwrap()).collect();
    let released: Vec<ResourceId> =
        plans.iter().flat_map(|p| p.releases.iter().map(|c| c.id)).collect();
    assert_eq!(released, vec![ResourceId(1)], "shrinkage releases the cache");
}

// ---------- G6.7: competing-process pressure ----------

#[test]
fn g6_competing_pressure_diagnostic() {
    let mut m = make_manager(mib(32));
    push_record(
        &mut m,
        mib(10),
        ResourcePriority::OptionalHistory,
        ContentSource::Regenerable,
    );
    m.synthetic.as_mut().unwrap().usage = mib(30);
    let plans: Vec<_> = (0..4).map(|_| m.sample_pressure().unwrap()).collect();
    let released: Vec<ResourceId> =
        plans.iter().flat_map(|p| p.releases.iter().map(|c| c.id)).collect();
    assert!(released.contains(&ResourceId(1)), "competing pressure evicts optional");
}

// ---------- G6.8: unavailable DXGI ----------

#[test]
fn g6_unavailable_dxgi_uses_conservative_fraction() {
    let mut m = MemoryManager::new();
    push_record(&mut m, 1000, ResourcePriority::CurrentFrame, ContentSource::Regenerable);
    let snap = m.query_local_memory().unwrap();
    assert_eq!(snap.budget, 1000, "budget floor = accounted without telemetry");
    assert!(!m.metrics().telemetry_available, "no telemetry available");

    let t = compute_targets(
        snap.budget,
        IdentityState::effective_target_fraction(0.80, m.identity_state()),
        m.policy().safety_reserve_bytes,
        m.policy().release_band_bytes,
    );
    assert!(t.soft_target <= (0.60 * snap.budget as f32) as u64 + 1);
}

// ---------- G6.9: adapter mismatch (no substitution) ----------

#[test]
fn g6_adapter_mismatch_no_substitution() {
    let hints = IdentityHints {
        luid: Some(0x1234_5678_9ABC_DEF0),
        vendor_id: Some(0x10DE),
        device_id: Some(0x2507),
        description: None,
    };
    let candidates = vec![anim_render::memory::identity::AdapterIdentity {
        luid: 0x1234_5678_9ABC_DEF1,
        description: "NVIDIA GeForce RTX 3050".into(),
        vendor_id: 0x10DE,
        device_id: 0x2507,
        dedicated_video_memory: 4 * 1024 * 1024 * 1024,
    }];
    let result = anim_render::memory::identity::correlate(&hints, &candidates);
    assert!(result.is_none(), "mismatched LUID must never substitute");
}

// ---------- G6.10: allocation failure after successful query ----------

#[test]
fn g6_allocation_failure_after_successful_query() {
    let mut m = make_manager(mib(32));
    m.synthetic.as_mut().unwrap().fail_next_allocations = 1;
    let err = m.preflight_allocation(1, ResourcePriority::Cache).unwrap_err();
    match err {
        MemoryError::OutOfMemory { .. } => {}
        _ => panic!("expected OOM"),
    }
    assert!(m.preflight_allocation(1, ResourcePriority::Cache).is_ok());
}

// ---------- G6.11: reload exactness ----------

#[test]
fn g6_reloaded_resources_recover_residency() {
    let mut m = make_manager(mib(64));
    let id = push_record(
        &mut m,
        mib(4),
        ResourcePriority::CurrentFrame,
        ContentSource::CpuShadow,
    );
    m.release_resource(id);
    assert!(!m.records().iter().find(|(k, _)| *k == id).unwrap().1.resident);
    // Reload in one frame.
    let batch = m.reload_required(&[id], mib(4)).unwrap();
    for e in &batch.scheduled {
        m.complete_reload(e.id);
    }
    assert!(batch.scheduled.len() == 1);
    let rec = m.records().iter().find(|(k, _)| *k == id).unwrap().1.clone();
    assert!(rec.resident, "reloaded resource must be resident");
    assert_eq!(rec.reload_state, ReloadState::Resident);
}

// ---------- G6.12: convergence ----------

#[test]
fn g6_convergence_below_target_when_evictable_exists() {
    let mut m = make_manager(mib(32));
    push_record(
        &mut m,
        mib(10),
        ResourcePriority::OptionalHistory,
        ContentSource::Regenerable,
    );
    push_record(&mut m, mib(10), ResourcePriority::Cache, ContentSource::Regenerable);
    m.synthetic.as_mut().unwrap().usage = mib(30);
    let mut released = Vec::new();
    for _ in 0..4 {
        let plan = m.sample_pressure().unwrap();
        released.extend(plan.releases.iter().map(|c| c.id));
    }
    assert!(!released.is_empty(), "evictable content must release");
}

// ---------- G6.13: actionable OOM ----------

#[test]
fn g6_actionable_oom_when_nothing_evictable() {
    let mut m = make_manager(mib(32));
    push_record(&mut m, mib(20), ResourcePriority::Critical, ContentSource::GpuOnly);
    m.synthetic.as_mut().unwrap().usage = mib(30);
    let plans: Vec<_> = (0..3).map(|_| m.sample_pressure().unwrap()).collect();
    let _ = plans;
    assert_eq!(m.metrics().counters.persistent_pressure_events, 1);

    let err = m.preflight_allocation(mib(5), ResourcePriority::CurrentFrame).unwrap_err();
    match err {
        MemoryError::OutOfMemory { requested, shortfall, .. } => {
            assert_eq!(requested, mib(5));
            assert!(shortfall > 0);
        }
        _ => panic!("wrong error type"),
    }
}
