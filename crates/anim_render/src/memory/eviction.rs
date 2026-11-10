//! Budget arithmetic and eviction planning (spec §16.7 Algorithms).
//!
//! All arithmetic is saturating (§16.7 soft_target clause). The eviction
//! plan is PURE: it takes a snapshot + registry view + completed serial
//! and returns the ordered release list — enforcement happens in the
//! allocator/orchestrator, keeping this module trivially unit-testable
//! against synthetic snapshots (§16.12).

use super::fences::FenceTracker;
use super::resource::{ContentSource, ResourceId, ResourcePriority, ResourceRecord};
use std::collections::BTreeSet;

/// Immutable budget snapshot (spec §16.8 `BudgetSnapshot`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetSnapshot {
    /// Observed-or-defaulted local-segment budget (bytes).
    pub budget: u64,
    /// Observed usage (DXGI when available; else 0).
    pub current_usage: u64,
    /// Sum of outstanding uncommitted reservations.
    pub current_reservation: u64,
    /// budget - current_reservation - current_usage (saturating).
    pub available_for_reservation: u64,
    /// Internal accounted bytes (registry sum of resident records).
    pub accounted_usage: u64,
    /// max(current_usage, accounted_usage) — conservative policy input.
    pub effective_usage: u64,
}

/// Target arithmetic per §16.6/§16.7 (all inside one function so the
/// golden tests pin the EXACT formula).
pub struct Targets {
    /// min(budget·target_fraction, budget − safety_reserve), saturating.
    pub soft_target: u64,
    /// soft_target − release_band, saturating — eviction stops here.
    pub release_watermark: u64,
}

pub fn compute_targets(
    budget: u64,
    policy_fraction: f32,
    reserve: u64,
    release_band: u64,
) -> Targets {
    let fraction_part = ((budget as f32 * policy_fraction) as u64).min(budget);
    let reserve_part = budget.saturating_sub(reserve);
    let soft_target = fraction_part.min(reserve_part);
    let release_watermark = soft_target.saturating_sub(release_band);
    Targets { soft_target, release_watermark }
}

/// A planned release (immutable plan entry; id is a registry key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvictCandidate {
    pub id: ResourceId,
    pub bytes: u64,
    pub reason: EvictionReason,
}

/// Why a candidate was chosen (diagnostics only; never ownership proof).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvictionReason {
    /// Lower-value priority class under pressure.
    LowPriority,
}

/// Immutable eviction plan output.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EvictionPlan {
    /// Ordered release list (§16.7 scoring order).
    pub releases: Vec<EvictCandidate>,
    /// effective_usage − sum(released bytes), saturating projection.
    pub projected_usage: u64,
    /// true when the projection satisfies the release watermark.
    pub satisfied: bool,
}

/// Reload-cost class ordering per §16.7: disk-backed/regenerable first,
/// then CPU-shadowed; unrestorable GPU-only is structurally excluded by
/// the filter below, and restorable GPU-only goes last.
pub(crate) fn source_score(source: &ContentSource) -> u8 {
    match source {
        ContentSource::Disk(_) => 0,
        ContentSource::Regenerable => 1,
        ContentSource::CpuShadow => 2,
        // Restorable GPU-only content (device-loss-rebuildable) is the
        // most expensive reload class present in candidate scope.
        ContentSource::GpuOnlyWithRestore => u8::MAX - 1,
        // Never scored — filtered out before any sort.
        ContentSource::GpuOnly => u8::MAX,
    }
}

/// Plan a bounded eviction set (spec §16.7 "Eviction plan").
///
/// Excluded: locked, mapped, required, unrestorable GPU-only, and
/// resources with `last_used_serial > completed_serial` (in flight).
/// Ordering: source class (disk/regenerable first), then priority
/// ascending (least valuable first), oldest `last_used_serial`, lowest
/// RL cost (bytes), stable `ResourceId` tie-break. The scan stops as
/// soon as the projection satisfies `release_watermark`.
pub(crate) fn plan_eviction(
    snapshot: &BudgetSnapshot,
    records: &[(ResourceId, ResourceRecord)],
    fences: &FenceTracker,
    required: &BTreeSet<ResourceId>,
    release_watermark: u64,
) -> EvictionPlan {
    let mut candidates: Vec<(u8, u8, ResourcePriority, u64, ResourceId, &ResourceRecord)> =
        Vec::new();

    for (id, rec) in records {
        // ---- exclusion filters (§16.7 literal list) ----
        if rec.locked {
            continue; // locked ⇒ mapped/readback-pinned in MVP semantics
        }
        if required.contains(id) {
            continue;
        }
        if !fences.is_settled(rec.last_used_serial) {
            continue; // in flight: last_used > completed
        }
        if matches!(rec.source, ContentSource::GpuOnly) {
            continue; // unrestorable: never released while registered
        }
        if rec.reload_state == super::resource::ReloadState::Reloading {
            continue; // mid-reload transfer in progress
        }
        if !rec.resident {
            continue; // already out — a second "release" is a no-op that
                      // must not re-enter plans or inflate metrics
        }

        candidates.push((
            source_score(&rec.source),
            priority_rank_for_eviction(&rec.priority),
            rec.priority,
            rec.last_used_serial,
            *id,
            rec,
        ));
    }

    // §16.7 order: prefer cheap sources, then ascending priority
    // (least-protective first), oldest last-used serial, lowest reload
    // cost (bytes), stable ResourceId tie-break.
    candidates.sort_by(|a, b| {
        (a.0, a.1, a.3, a.5.bytes, a.4).cmp(&(b.0, b.1, b.3, b.5.bytes, b.4))
    });

    let mut plan = EvictionPlan {
        projected_usage: snapshot.effective_usage,
        satisfied: snapshot.effective_usage <= release_watermark,
        ..EvictionPlan::default()
    };

    if plan.satisfied {
        return plan; // no release needed
    }

    for (_, _, _, _, id, rec) in &candidates {
        plan.releases.push(EvictCandidate {
            id: *id,
            bytes: rec.bytes,
            reason: EvictionReason::LowPriority,
        });
        plan.projected_usage = plan.projected_usage.saturating_sub(rec.bytes);
        plan.satisfied = plan.projected_usage <= release_watermark;
        if plan.satisfied {
            break; // stop once the lower watermark is satisfied
        }
    }
    plan
}

/// Eviction-scanning rank: LEAST valuable numerically first (matches
/// `ResourcePriority` Ord already: OptionalHistory < ... < Critical).
pub(crate) fn priority_rank_for_eviction(p: &ResourcePriority) -> u8 {
    match p {
        ResourcePriority::OptionalHistory => 0,
        ResourcePriority::Cache => 1,
        ResourcePriority::NextFrame => 2,
        ResourcePriority::CurrentFrame => 3,
        ResourcePriority::Critical => 4,
    }
}

/// Candidate ordering is test-critical; exposed for the unit suite.
pub fn candidate_order(
    records: &[(ResourceId, ResourceRecord)],
    fences: &FenceTracker,
) -> Vec<ResourceId> {
    let snap = BudgetSnapshot {
        budget: u64::MAX,
        current_usage: u64::MAX,
        current_reservation: 0,
        available_for_reservation: 0,
        accounted_usage: u64::MAX,
        effective_usage: u64::MAX,
    };
    // Watermark 0 with MAX usage: never satisfied up-front, so the plan
    // enumerates EVERY releasable candidate in scoring order.
    let plan = plan_eviction(&snap, records, fences, &BTreeSet::new(), 0);
    plan.releases.into_iter().map(|c| c.id).collect()
}

#[cfg(test)]
mod tests {
    use super::super::resource::ReloadState;
    use super::*;

    fn rec(
        id: u64,
        bytes: u64,
        priority: ResourcePriority,
        source: ContentSource,
    ) -> ResourceRecord {
        ResourceRecord {
            id: ResourceId::PLACEHOLDER,
            label: format!("r{id}"),
            bytes,
            source,
            priority,
            last_used_serial: 0,
            last_used_frame: 0,
            resident: true,
            locked: false,
            reload_state: ReloadState::Resident,
        }
    }

    fn snap(budget: u64, usage: u64, accounted: u64) -> BudgetSnapshot {
        BudgetSnapshot {
            budget,
            current_usage: usage,
            current_reservation: 0,
            available_for_reservation: budget.saturating_sub(usage),
            accounted_usage: accounted,
            effective_usage: usage.max(accounted),
        }
    }

    #[test]
    fn targets_are_saturating_min_of_fraction_and_reserve() {
        // budget 4 GiB, fraction 0.8 → f32 multiply yields 3435973888
        // (0.8f32 = 0.8000000119…; the +51-byte rounding is a PINNED
        // artifact of the §16.7 formula cast — changing it is a golden
        // event, not a silent fix).
        let t = compute_targets(
            4 * 1024 * 1024 * 1024,
            0.80,
            128 * 1024 * 1024,
            64 * 1024 * 1024,
        );
        // 0.8·memory = 3435973888 < budget−128MiB → min takes the fraction part.
        assert_eq!(t.soft_target, 3435973888);
        assert_eq!(t.release_watermark, t.soft_target - 64 * 1024 * 1024);

        // Exact-math cross-check that the min really binds: with a huge
        // reserve the reserve part wins.
        let t2 = compute_targets(100, 0.80, 51, 40);
        // fraction_part 80; reserve_part 49 → min 49; watermark 9.
        assert_eq!(t2.soft_target, 49);
        assert_eq!(t2.release_watermark, 9);

        // Reserve larger than budget: soft target collapses to 0, never panics.
        let t3 = compute_targets(100, 0.80, 1_000_000, 50);
        assert_eq!(t3.soft_target, 0);
        assert_eq!(t3.release_watermark, 0);
    }

    #[test]
    fn effective_usage_takes_max_of_observed_and_accounted() {
        let s = snap(1000, 500, 700);
        assert_eq!(s.effective_usage, 700);
        let s2 = snap(1000, 900, 100);
        assert_eq!(s2.effective_usage, 900);
    }

    #[test]
    fn exclusions_block_locked_required_inflight_and_gpu_only() {
        let fences = FenceTracker::new();
        let mut records = vec![(
            ResourceId(1),
            rec(1, 100, ResourcePriority::OptionalHistory, ContentSource::Regenerable),
        )];

        // locked excluded
        let mut locked =
            rec(2, 100, ResourcePriority::OptionalHistory, ContentSource::Regenerable);
        locked.locked = true;
        records.push((ResourceId(2), locked));

        // required excluded
        records.push((
            ResourceId(3),
            rec(3, 100, ResourcePriority::OptionalHistory, ContentSource::Regenerable),
        ));

        // in-flight excluded (last_used > completed: nothing completed yet, serial 1 used)
        let mut inflight =
            rec(4, 100, ResourcePriority::OptionalHistory, ContentSource::Regenerable);
        inflight.last_used_serial = 1;
        records.push((ResourceId(4), inflight));

        // unrestorable GPU-only excluded forever
        records.push((
            ResourceId(5),
            rec(5, 100, ResourcePriority::OptionalHistory, ContentSource::GpuOnly),
        ));

        // mid-reload excluded
        let mut reloading =
            rec(6, 100, ResourcePriority::OptionalHistory, ContentSource::Regenerable);
        reloading.reload_state = ReloadState::Reloading;
        records.push((ResourceId(6), reloading));

        let mut required = BTreeSet::new();
        required.insert(ResourceId(3));

        let plan = plan_eviction(&snap(1000, 600, 0), &records, &fences, &required, 0);
        let ids: Vec<u64> = plan.releases.iter().map(|c| c.id.0).collect();
        assert_eq!(
            ids,
            vec![1],
            "only the unlocked, non-required, settled, restorable id 1 may release"
        );
    }

    #[test]
    fn ordering_prefers_source_class_priority_serial_bytes_id() {
        let mut fences5 = FenceTracker::new();
        let records: Vec<(ResourceId, ResourceRecord)> = vec![
            // disk (cheapest reload class), serial 2
            (
                ResourceId(1),
                rec(
                    1,
                    100,
                    ResourcePriority::OptionalHistory,
                    ContentSource::Disk(anim_core::id::AssetId::from_u128(1)),
                ),
            ),
            // CpuShadow cache; heavier reload class than disk/regenerable
            (ResourceId(2), rec(2, 100, ResourcePriority::Cache, ContentSource::CpuShadow)),
            // Regenerable pair, newer serial 5 → after disk, by id 3 < 4
            (
                ResourceId(3),
                rec(3, 100, ResourcePriority::OptionalHistory, ContentSource::Regenerable),
            ),
            (
                ResourceId(4),
                rec(4, 100, ResourcePriority::OptionalHistory, ContentSource::Regenerable),
            ),
        ];
        let regs: Vec<(ResourceId, ResourceRecord)> = records
            .into_iter()
            .map(|(id, mut r)| {
                r.last_used_serial = if id == ResourceId(1) { 2 } else { 5 };
                (id, r)
            })
            .collect();
        fences5.record_completed(5); // settles serials 2 and 5

        let order = candidate_order(&regs, &fences5);
        // §16.7 scan order: source class FIRST (disk 0 < regenerable 1 <
        // cpu_shadow 2), then priority, then serial, bytes, id.
        assert_eq!(
            order,
            vec![ResourceId(1), ResourceId(3), ResourceId(4), ResourceId(2)],
            "disk first, then the regenerable pair, CPU-shadow cache last"
        );
    }

    #[test]
    fn plan_stops_at_release_watermark() {
        let fences = FenceTracker::new();
        let records: Vec<(ResourceId, ResourceRecord)> = (1u64..=5)
            .map(|i| {
                (
                    ResourceId(i),
                    rec(
                        i,
                        100,
                        ResourcePriority::OptionalHistory,
                        ContentSource::Regenerable,
                    ),
                )
            })
            .collect();

        // Effective 500; watermark 380 → need to release ≥ 120, releases 2.
        let plan =
            plan_eviction(&snap(1000, 500, 0), &records, &fences, &BTreeSet::new(), 380);
        assert_eq!(plan.releases.len(), 2);
        assert!(plan.satisfied);
        assert_eq!(plan.projected_usage, 500 - 200);
    }

    #[test]
    fn insufficient_candidates_reports_unsatisfied_not_panic() {
        let fences = FenceTracker::new();
        let records = vec![(
            ResourceId(1),
            rec(1, 100, ResourcePriority::CurrentFrame, ContentSource::Regenerable),
        )];
        let plan =
            plan_eviction(&snap(1000, 500, 0), &records, &fences, &BTreeSet::new(), 0);
        assert!(!plan.satisfied);
        assert_eq!(plan.releases.len(), 1);
        assert_eq!(plan.projected_usage, 400);
    }

    #[test]
    fn asset_id_from_u128_legacy_helper_exists() {
        // Anchor: keeps the disk-source test compiling when AssetId
        // construction API changes.
        assert_ne!(
            anim_core::id::AssetId::from_u128(1),
            anim_core::id::AssetId::from_u128(0)
        );
    }
}
