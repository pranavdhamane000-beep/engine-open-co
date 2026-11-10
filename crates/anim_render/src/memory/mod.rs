//! Public memory API and policy orchestration (spec §16.3 mod.rs).
//!
//! `MemoryManager` is owned by anim_render and consumed by
//! application/export diagnostics (§16.4). It composes:
//! - `identity` — correlated adapter identity (pure state machine)
//! - `resource` — registry of explicit-size records (§16.8)
//! - `fences`   — submission/completion serials (§16.7)
//! - `eviction` — snapshot math + candidate scoring (§16.7)
//! - `allocator`— preflight/reservations/rollback (§16.7)
//! - `streaming`— throttled reload queues (§16.7)
//! - `recovery` — OOM degradation + device-loss reconstruction
//! - `metrics`  — immutable snapshots
//!
//! DXGI telemetry (dxgi.rs, chunk 2) is ADVISORY: internal accounting
//! and actual allocation errors remain authoritative everywhere.
//! Differences between observed and accounted usage are diagnostics,
//! never proof of ownership by another process (§16.7).

pub mod allocator;
pub mod dxgi;
pub mod eviction;
pub mod fences;
pub mod identity;
pub mod metrics;
pub mod recovery;
pub mod resource;
pub mod streaming;

use eviction::BudgetSnapshot;
pub use fences::FenceTracker;
pub use identity::{AdapterIdentity, IdentityHints, IdentityState};
use metrics::{MetricCounters, MetricsSnapshot};
use resource::{
    ContentSource, MemoryPolicy, ReloadState, ResourceId, ResourcePriority, ResourceRecord,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::time::Instant;
use streaming::{ReloadBatch, StreamingQueue};

/// Typed error set (§16.11 shapes; converted to the crate-level §15.11
/// family with `From` at the bottom of this file).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MemoryError {
    /// Identity could not be correlated confidently (§16.6) — the
    /// manager still functions with internal accounting and a
    /// conservative target.
    #[error("memory telemetry unavailable: {0}")]
    TelemetryUnavailable(String),
    /// DXGI query failed transiently; identity may be stale (§16.11).
    #[error("adapter memory query failed: {0}")]
    QueryFailed(String),
    /// The resolved identity no longer matches the rendering device.
    #[error("stale adapter identity: {0}")]
    StaleIdentity(String),
    /// Actionable OOM after bounded recovery (§16.11 byte context).
    #[error("out of memory: requested {requested} bytes; soft target {budget}; effective usage {effective_usage} (+{reservation_total} reserved); over by {shortfall}: {detail}")]
    OutOfMemory {
        requested: u64,
        budget: u64,
        effective_usage: u64,
        reservation_total: u64,
        shortfall: u64,
        detail: String,
    },
    /// Unknown id in a manager call (caller fabrication).
    #[error("unknown resource: {0:?}")]
    UnknownResource(ResourceId),
    /// Validation/duplicate/registry-shape errors.
    #[error("invalid memory record: {0}")]
    InvalidRecord(String),
    /// Device lost mid-operation; residency must be rebuilt (§16.11).
    #[error("device loss invalidates residency: {0}")]
    DeviceLost(String),
}

impl MemoryError {
    /// Crate-family bridge (used by device wiring; `OutOfMemory` keeps
    /// its full context string in the §15.11 variant).
    pub fn to_render_error(&self) -> crate::error::RenderError {
        use crate::error::RenderError;
        match self {
            Self::DeviceLost(_) => RenderError::DeviceLost,
            other => RenderError::OutOfMemory(other.to_string()),
        }
    }
}

/// Synthetic budget source (test + fault-injection surface; NEVER set
/// in production paths — anim_test_support::gpu_memory is the setter).
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntheticBudget {
    pub budget: u64,
    pub usage: u64,
    /// When Some(n): the next n preflights FAIL with OOM regardless of
    /// arithmetic (fault injection, §16.12).
    pub fail_next_allocations: u8,
}

/// Live telemetry source (§16.3 dxgi.rs is THE production impl; tests
/// may inject their own). Returns (budget, current_usage) of the
/// CORRELATED adapter's local segment (§16.6: never another adapter).
pub trait BudgetTelemetry {
    fn budget_and_usage(&mut self) -> Result<(u64, u64), MemoryError>;
}

impl SyntheticBudget {
    pub fn new(budget: u64, usage: u64) -> Self {
        Self { budget, usage, fail_next_allocations: 0 }
    }
}

/// Public manager (construction via `new` / `with_policy` / hints).
pub struct MemoryManager {
    policy: MemoryPolicy,
    /// Caller-side correlation inputs (resolved in chunk-2 dxgi.rs; the
    /// engine NEVER requests devices itself — D-11 preserved).
    hints: IdentityHints,
    identity: IdentityState,
    registry: BTreeMap<ResourceId, ResourceRecord>,
    fences: FenceTracker,
    ledger: allocator::SharedLedger,
    streaming: StreamingQueue,
    counters: MetricCounters,
    /// Monotonic metrics/snapshot generation.
    generation: u64,
    last_snapshot: Option<BudgetSnapshot>,
    last_query: Option<Instant>,
    /// Consecutive over-budget samples (§16.6 hysteresis).
    over_budget_count: u8,
    /// Latched after the consecutive threshold; eviction CONTINUES
    /// below the release watermark afterwards (§16.6).
    evicting: bool,
    /// Live DXGI telemetry (chunk 2: `dxgi::resolve` attaches; only the
    /// CORRELATED adapter — §16.6). None = estimate-only operation.
    telemetry: Option<Box<dyn BudgetTelemetry>>,
    /// Hermetic telemetry override (doc(hidden) diagnostics hook).
    #[doc(hidden)]
    pub synthetic: Option<SyntheticBudget>,
}

impl MemoryManager {
    pub fn new() -> Self {
        Self::with_policy(MemoryPolicy::default())
    }

    pub fn with_policy(policy: MemoryPolicy) -> Self {
        Self::with_policy_and_hints(policy, IdentityHints::default())
    }

    /// Wire caller-side identity hints now; actual correlation against
    /// DXGI enumeration happens at resolve time (chunk 2).
    pub fn with_policy_and_hints(policy: MemoryPolicy, hints: IdentityHints) -> Self {
        Self {
            policy,
            hints,
            identity: IdentityState::Unprobed,
            registry: BTreeMap::new(),
            fences: FenceTracker::new(),
            ledger: Rc::new(RefCell::new(allocator::ReservationLedger::default())),
            streaming: StreamingQueue::default(),
            counters: MetricCounters::default(),
            generation: 0,
            last_snapshot: None,
            last_query: None,
            over_budget_count: 0,
            evicting: false,
            telemetry: None,
            synthetic: None,
        }
    }

    /// This manager's hints (diagnostics; chunk-2 resolve path).
    pub fn hints(&self) -> &IdentityHints {
        &self.hints
    }

    /// Attach a live telemetry source (the CORRELATED adapter only —
    /// §16.6; `dxgi::resolve` builds it). Replaces any previous source.
    pub fn set_telemetry(&mut self, source: Box<dyn BudgetTelemetry>) {
        self.telemetry = Some(source);
    }

    // ---------------- §16.9 interface surface ----------------

    /// §16.9 `query_local_memory`. With telemetry available this is the
    /// DXGI local-segment view refreshed within the policy interval;
    /// otherwise internal estimates. No identity → budget floor =
    /// accounted + reservations (the only honest size claim).
    pub fn query_local_memory(&mut self) -> Result<BudgetSnapshot, MemoryError> {
        // Advisory honoring of the query interval (§16.8).
        let interval_ok = self
            .last_query
            .map(|t| t.elapsed() >= self.policy.query_interval)
            .unwrap_or(true);

        let (budget, observed): (u64, u64) = if let Some(s) = self.synthetic {
            (s.budget, s.usage)
        } else if self.telemetry_available() {
            if let Some(src) = self.telemetry.as_mut() {
                if interval_ok {
                    // THE live query: correlated adapter's local segment
                    // (§16.6 dxgi.rs). DXGI is ADVISORY — a failed query
                    // falls back to the prior sample, monthly counters
                    // record it, and internal accounting stays
                    // authoritative (§16.11).
                    match src.budget_and_usage() {
                        Ok((b, u)) => {
                            self.counters.telemetry_samples =
                                self.counters.telemetry_samples.saturating_add(1);
                            (b, u)
                        }
                        Err(e) => {
                            self.counters.telemetry_errors =
                                self.counters.telemetry_errors.saturating_add(1);
                            let _ = e;
                            self.last_snapshot
                                .map(|s| (s.budget, s.current_usage))
                                .unwrap_or((0, 0))
                        }
                    }
                } else {
                    // Inside the query interval: keep the prior sample.
                    match self.last_snapshot {
                        Some(s) => (s.budget, s.current_usage),
                        None => (0, 0),
                    }
                }
            } else {
                // Identity resolved but no live source attached (e.g.
                // IDXGIAdapter3 cast failed): dedicated-memory estimate.
                match self.identity {
                    IdentityState::Resolved(ref id) => (id.dedicated_video_memory, 0),
                    _ => (0, 0),
                }
            }
        } else {
            (0, 0)
        };

        let accounted = self.accounted_usage();
        let reservation = self.ledger.borrow().total_open_bytes();
        let effective = observed.max(accounted).max(reservation);
        // No telemetry → accounted+reservation is the honest floor.
        let budget_final =
            if budget == 0 { accounted.saturating_add(reservation) } else { budget };

        self.last_query = Some(Instant::now());
        if self.synthetic.is_some() {
            self.counters.telemetry_samples =
                self.counters.telemetry_samples.saturating_add(1);
        }
        let snap = BudgetSnapshot {
            budget: budget_final,
            current_usage: observed,
            current_reservation: reservation,
            available_for_reservation: budget_final
                .saturating_sub(observed)
                .saturating_sub(reservation),
            accounted_usage: accounted,
            effective_usage: effective,
        };
        self.last_snapshot = Some(snap);
        self.generation = self.generation.wrapping_add(1);
        Ok(snap)
    }

    /// §16.9 `register_resource`. Validates, then assigns the REAL
    /// monotonic id (record.id placeholder is overwritten; see
    /// resource.rs). Unrestorable GpuOnly records are legal to register
    /// (they model already-owned residency) but can never reload.
    pub fn register_resource(
        &mut self,
        mut record: ResourceRecord,
    ) -> Result<ResourceId, MemoryError> {
        record.validate().map_err(MemoryError::InvalidRecord)?;
        let next = self.registry.keys().next_back().map(|k| k.0 + 1).unwrap_or(1);
        record.id = ResourceId(next);
        let id = record.id;
        if self.registry.insert(id, record).is_some() {
            // Unreachable with monotonic assignment; kept as the
            // documented guard for future explicit-id support.
            return Err(MemoryError::InvalidRecord(format!("duplicate id {id:?}")));
        }
        Ok(id)
    }

    /// §16.9 `mark_used(id, serial, frame)`. Unknown ids are silent by
    /// construction (callers pass ids from registration; test-verified).
    pub fn mark_used(&mut self, id: ResourceId, serial: u64, frame: u64) {
        if let Some(rec) = self.registry.get_mut(&id) {
            rec.last_used_serial = rec.last_used_serial.max(serial);
            rec.last_used_frame = rec.last_used_frame.max(frame);
        }
    }

    /// §16.9 `on_submitted(serial)` — fence lifecycle step.
    pub fn on_submitted(&mut self, serial: u64) {
        self.fences.record_submitted(serial);
    }

    /// §16.9 `on_completed(serial)` — fence lifecycle step.
    pub fn on_completed(&mut self, serial: u64) {
        self.fences.record_completed(serial);
    }

    /// §16.9 `plan_eviction(snapshot, required)`.
    pub fn plan_eviction(
        &self,
        snapshot: &BudgetSnapshot,
        required: &BTreeSet<ResourceId>,
    ) -> eviction::EvictionPlan {
        let t = self.targets(snapshot);
        let items: Vec<(ResourceId, ResourceRecord)> =
            self.registry.iter().map(|(k, v)| (*k, v.clone())).collect();
        eviction::plan_eviction(
            snapshot,
            &items,
            &self.fences,
            required,
            t.release_watermark,
        )
    }

    /// §16.9 `preflight_allocation(bytes, priority)` — permit with
    /// §16.8 drop/commit semantics (allocator.rs). Allocation failure
    /// (fault hook here; real wgpu failure in chunk 2 wiring) bypasses
    /// sampling delays by returning immediately.
    pub fn preflight_allocation(
        &mut self,
        bytes: u64,
        priority: ResourcePriority,
    ) -> Result<allocator::AllocationPermit, MemoryError> {
        let snapshot = self.query_local_memory()?;

        // Fault-injection hook (bounded, explicit, diagnostic-only).
        if let Some(s) = self.synthetic.as_mut() {
            if s.fail_next_allocations > 0 {
                s.fail_next_allocations -= 1;
                self.counters.allocation_refusals += 1;
                return Err(MemoryError::OutOfMemory {
                    requested: bytes,
                    budget: snapshot.budget,
                    effective_usage: snapshot.effective_usage,
                    reservation_total: 0,
                    shortfall: bytes,
                    detail: "fault-injected allocation failure (test hook)".into(),
                });
            }
        }

        let t = self.targets(&snapshot);
        let reservation_open = self.ledger.borrow().total_open_bytes();
        let plan_view = self.plan_eviction(&snapshot, &BTreeSet::new());
        match allocator::preflight(
            &snapshot,
            reservation_open,
            bytes,
            t.soft_target,
            &plan_view,
        ) {
            allocator::PreflightOutcome::Grant => {
                let id = self.ledger.borrow_mut().reserve(bytes, priority);
                self.counters.allocations_granted += 1;
                Ok(self.new_permit(id, bytes, priority))
            }
            allocator::PreflightOutcome::GrantAfterEviction { plan } => {
                // The DEVICE layer applies the releases before its real
                // allocation (chunk 2); bookkeeping matches Grant.
                if !plan.releases.is_empty() {
                    self.counters.evictions =
                        self.counters.evictions.saturating_add(plan.releases.len() as u64);
                }
                let id = self.ledger.borrow_mut().reserve(bytes, priority);
                self.counters.allocations_granted += 1;
                Ok(self.new_permit(id, bytes, priority))
            }
            allocator::PreflightOutcome::Refuse { refused } => {
                self.counters.allocation_refusals += 1;
                Err(refused.into_memory_error("preflight refused within bounded policy"))
            }
        }
    }

    fn new_permit(
        &self,
        id: allocator::ReservationId,
        bytes: u64,
        priority: ResourcePriority,
    ) -> allocator::AllocationPermit {
        allocator::AllocationPermit::shared(self.ledger.clone(), id, bytes, priority)
    }

    /// §16.9 `reload_required(required, frame_budget)`.
    pub fn reload_required(
        &mut self,
        required: &[ResourceId],
        frame_budget: u64,
    ) -> Result<ReloadBatch, MemoryError> {
        for id in required {
            if !self.registry.contains_key(id) {
                return Err(MemoryError::UnknownResource(*id));
            }
        }
        let look =
            |id: ResourceId| self.registry.get(&id).map(|r| (r.reload_state, r.bytes));
        let batch = self.streaming.plan_reload_batch(required, &look, frame_budget);
        for e in &batch.scheduled {
            if let Some(r) = self.registry.get_mut(&e.id) {
                if r.reload_state == ReloadState::PendingReload {
                    r.reload_state = ReloadState::Reloading;
                }
            }
        }
        self.counters.reload_batches += 1;
        if batch.backed_up {
            self.counters.reload_stalls += 1;
        }
        Ok(batch)
    }

    /// Device layer calls after a scheduled upload COPY lands: the
    /// record goes Resident again (§16.8 reload-transition mutation).
    pub fn complete_reload(&mut self, id: ResourceId) {
        if let Some(r) = self.registry.get_mut(&id) {
            r.reload_state = ReloadState::Resident;
            r.resident = true;
        }
    }

    /// Lock a record (mapped/readback-pinned): never released nor
    /// evicted while locked (§16.6).
    pub fn set_locked(&mut self, id: ResourceId, locked: bool) -> Result<(), MemoryError> {
        let rec = self.registry.get_mut(&id).ok_or(MemoryError::UnknownResource(id))?;
        rec.locked = locked;
        Ok(())
    }

    // ---------------- pressure / hysteresis (§16.6) ----------------

    /// One pressure sample. Consecutive over-budget samples ≥ policy
    /// count latch eviction ON; release continues below the release
    /// watermark. Returns the plan APPLIED this sample (empty allowed).
    pub fn sample_pressure(&mut self) -> Result<eviction::EvictionPlan, MemoryError> {
        let snapshot = self.query_local_memory()?;
        let frac = self.target_fraction_for_state();
        let t = eviction::compute_targets(
            snapshot.budget,
            frac,
            self.policy.safety_reserve_bytes,
            self.policy.release_band_bytes,
        );

        if snapshot.effective_usage > t.soft_target {
            self.over_budget_count = self.over_budget_count.saturating_add(1);
        } else if snapshot.effective_usage <= t.release_watermark {
            // Back under the LOWER band: full reset (§16.6 "continues
            // below a lower release watermark").
            self.over_budget_count = 0;
            self.evicting = false;
        }
        if self.over_budget_count >= self.policy.over_budget_samples {
            self.evicting = true;
        }
        if !self.evicting {
            return Ok(eviction::EvictionPlan::default());
        }

        let plan = self.apply_plan(&snapshot);
        if !plan.satisfied {
            // §16.11: persistent pressure = explicit diagnostic, never
            // false success. Covers BOTH "released but still over" and
            // "nothing releasable at all" (locked/GpuOnly-only).
            self.counters.persistent_pressure_events += 1;
        }
        Ok(plan)
    }

    /// Run an eviction plan's releases for real (registry transitions)
    /// and record metrics. Shared by `sample_pressure` and preflight.
    fn apply_plan(&mut self, snapshot: &BudgetSnapshot) -> eviction::EvictionPlan {
        let plan = self.plan_eviction(snapshot, &BTreeSet::new());
        if !plan.releases.is_empty() {
            for c in &plan.releases {
                self.release_resource(c.id);
            }
            self.counters.evictions =
                self.counters.evictions.saturating_add(plan.releases.len() as u64);
            self.counters.evicted_bytes = self
                .counters
                .evicted_bytes
                .saturating_add(plan.releases.iter().map(|c| c.bytes).sum::<u64>());
        }
        plan
    }

    /// Mark a record released (evicted): residency lost; reload pending
    /// for every restorable source class. GpuOnly NEVER reaches here
    /// (the eviction filter blocks it) — the guard keeps that invariant
    /// locally even if a future caller misuses `release_resource`.
    /// Public per §16.8's mutation list ("allocation, release, use
    /// marking, locking, fence completion and reload transition").
    pub fn release_resource(&mut self, id: ResourceId) {
        if let Some(r) = self.registry.get_mut(&id) {
            r.resident = false;
            r.reload_state = match r.source {
                ContentSource::GpuOnly => ReloadState::Resident,
                _ => ReloadState::PendingReload,
            };
        }
    }

    /// Preview degradation burst (§16.6 "reduce preview-only optional
    /// targets"): drop degradable classes regardless of hysteresis.
    pub fn degrade_preview(&mut self) -> usize {
        let ids: Vec<ResourceId> = self
            .registry
            .iter()
            .filter(|(_, r)| recovery::preview_degradable(r))
            .map(|(k, _)| *k)
            .collect();
        let freed = ids.len();
        for id in ids {
            self.release_resource(id);
        }
        self.counters.evictions = self.counters.evictions.saturating_add(freed as u64);
        freed
    }

    /// Device loss (§16.11): fences, ledger, streaming, residency all
    /// reset; content rebuilds via the reload queue from CPU/disk.
    /// Returns how many records need rebuilding.
    pub fn invalidate_for_device_loss(&mut self) -> usize {
        self.fences.reset_for_device_loss();
        self.ledger.borrow_mut().reset_for_device_loss();
        self.streaming.abort_in_flight();
        self.over_budget_count = 0;
        self.evicting = false;
        self.last_snapshot = None;
        self.generation = self.generation.wrapping_add(1);
        recovery::invalidate_for_device_loss(&mut self.registry)
    }

    /// Immutable metrics copy (§16.8).
    pub fn metrics(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            generation: self.generation,
            counters: self.counters,
            last_snapshot: self.last_snapshot,
            telemetry_available: self.telemetry_available(),
            registry_resident_bytes: self.accounted_usage(),
        }
    }

    /// §16.7 fence assignment point: serial for the NEXT queue
    /// submission. The submitter marks used resources with it, then
    /// calls `on_submitted` after `queue.submit`.
    pub fn begin_submission(&mut self) -> u64 {
        self.fences.assign_next()
    }

    /// Cloned registry view for diagnostics/report surfaces.
    pub fn records(&self) -> Vec<(ResourceId, ResourceRecord)> {
        self.registry.iter().map(|(k, v)| (*k, v.clone())).collect()
    }

    /// Highest submission serial recorded (diagnostics/tests).
    pub fn submitted_high(&self) -> u64 {
        self.fences.submitted_high()
    }

    /// Highest completion serial recorded (diagnostics/tests).
    pub fn completed_high(&self) -> u64 {
        self.fences.completed_high()
    }

    pub fn identity_state(&self) -> &IdentityState {
        &self.identity
    }

    /// Set the correlated identity directly (chunk-2 dxgi.rs and tests
    /// feed through here; `Unavailable` keeps telemetry off).
    pub fn set_identity(&mut self, state: IdentityState) {
        // Swapping IN a resolved identity is the CALLER's duty to back
        // with matching external data; the manager trusts nothing
        // about the machine beyond this record type.
        self.identity = state;
    }

    /// Read-only access to the memory policy.
    pub fn policy(&self) -> &MemoryPolicy {
        &self.policy
    }

    /// Whether eviction is currently latched.
    pub fn is_evicting(&self) -> bool {
        self.evicting
    }

    /// Shared ledger handle (for diagnostics/tests).
    pub fn ledger(&self) -> &allocator::SharedLedger {
        &self.ledger
    }

    /// Total open reservation bytes (diagnostics/tests).
    pub fn reservation_total(&self) -> u64 {
        self.ledger.borrow().total_open_bytes()
    }

    // ---------------- internals ----------------

    /// Telemetry availability = correlated identity, OR a live source,
    /// OR the diagnostic synthetic hook (which mirrors the §16.3
    /// test_support contract: injection behaves like a resolved adapter).
    fn telemetry_available(&self) -> bool {
        self.identity.telemetry_allowed()
            || self.telemetry.is_some()
            || self.synthetic.is_some()
    }

    /// §16.6 conservative fallback: configured fraction when telemetry
    /// exists, else clamped down to the pinned conservative floor.
    fn target_fraction_for_state(&self) -> f32 {
        if self.telemetry_available() {
            self.policy.target_fraction
        } else {
            self.policy
                .target_fraction
                .min(identity::defaults::CONSERVATIVE_TARGET_FRACTION)
        }
    }

    fn accounted_usage(&self) -> u64 {
        self.registry.values().filter(|r| r.resident).map(|r| r.bytes).sum()
    }

    fn targets(&self, snapshot: &BudgetSnapshot) -> eviction::Targets {
        eviction::compute_targets(
            snapshot.budget,
            self.target_fraction_for_state(),
            self.policy.safety_reserve_bytes,
            self.policy.release_band_bytes,
        )
    }
}

impl Default for MemoryManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Small-scale policy mirroring the default SHAPE (all fields are
    /// real; only byte magnitudes shrink). With the default 128 MiB
    /// reserve against a 1000-byte synthetic budget the soft target
    /// collapses to 0 — tests exercise arithmetic at this scale instead.
    fn test_policy() -> MemoryPolicy {
        MemoryPolicy {
            target_fraction: 0.80,
            safety_reserve_bytes: 100,
            release_band_bytes: 50,
            over_budget_samples: 3,
            query_interval: std::time::Duration::from_millis(0),
            max_upload_bytes_per_frame: 16 * 1024 * 1024,
        }
    }

    fn synthetic_manager(budget: u64) -> MemoryManager {
        let mut m = MemoryManager::with_policy(test_policy());
        m.synthetic = Some(SyntheticBudget::new(budget, 0));
        m
    }

    fn rec(
        bytes: u64,
        priority: ResourcePriority,
        source: ContentSource,
    ) -> ResourceRecord {
        ResourceRecord {
            id: ResourceId::PLACEHOLDER,
            label: format!("rec_{bytes}"),
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

    #[test]
    fn register_assigns_monotonic_ids_and_validates() {
        let mut m = synthetic_manager(1 << 20);
        let a = m
            .register_resource(rec(
                100,
                ResourcePriority::Cache,
                ContentSource::Regenerable,
            ))
            .unwrap();
        assert_eq!(a, ResourceId(1));
        let b = m
            .register_resource(rec(50, ResourcePriority::Cache, ContentSource::CpuShadow))
            .unwrap();
        assert_eq!(b, ResourceId(2));

        // Validation failure is typed and consumes no id.
        let mut bad = rec(0, ResourcePriority::Cache, ContentSource::Regenerable);
        bad.bytes = 0;
        assert!(matches!(m.register_resource(bad), Err(MemoryError::InvalidRecord(_))));
        let c = m
            .register_resource(rec(7, ResourcePriority::Cache, ContentSource::CpuShadow))
            .unwrap();
        assert_eq!(c, ResourceId(3));
    }

    #[test]
    fn hysteresis_latches_after_three_consecutive_and_holds_until_watermark() {
        let mut m = synthetic_manager(1000);
        m.register_resource(rec(
            700,
            ResourcePriority::OptionalHistory,
            ContentSource::Regenerable,
        ))
        .unwrap();

        // accounted 700 ≤ target 800: no pressure at all.
        let plan = m.sample_pressure().unwrap();
        assert!(plan.releases.is_empty());
        assert!(!m.evicting);

        // DXGI usage bump: effective 900 > 800 target (targets use the
        // conservative fraction ONLY without any telemetry; the
        // synthetic hook counts as available → configured 0.80).
        m.synthetic.as_mut().unwrap().usage = 900;
        for i in 0..2 {
            let plan = m.sample_pressure().unwrap();
            assert!(plan.releases.is_empty(), "sample {i}: below the 3-sample threshold");
        }
        assert!(!m.evicting);

        let plan = m.sample_pressure().unwrap();
        assert!(m.evicting, "3rd consecutive sample latches");
        assert_eq!(plan.releases.len(), 1);
        assert_eq!(plan.projected_usage, 200);
        assert!(plan.satisfied);
        assert!(m.registry.values().all(|r| !r.resident));

        // Hysteresis: release work continues while between target and
        // release watermark even with nothing left to evict (760 sits
        // in band: not over target 800, not under watermark 750).
        m.synthetic.as_mut().unwrap().usage = 760;
        m.sample_pressure().unwrap();
        assert!(m.evicting, "760 > watermark 750 → still latched");

        // Below the lower watermark: full reset.
        m.synthetic.as_mut().unwrap().usage = 10;
        m.sample_pressure().unwrap();
        assert!(!m.evicting);
    }

    #[test]
    fn conservative_fraction_applies_without_telemetry() {
        // No synthetic (no telemetry path at all): budget floor ==
        // accounted, fraction forced to 0.60.
        let mut m = MemoryManager::new();
        m.register_resource(rec(
            300,
            ResourcePriority::CurrentFrame,
            ContentSource::Regenerable,
        ))
        .unwrap();
        let snap = m.query_local_memory().unwrap();
        assert_eq!(snap.budget, 300);
        assert_eq!(snap.effective_usage, 300);
        assert!(!m.metrics().telemetry_available);
    }

    #[test]
    fn eviction_never_takes_inflight_locked_required_or_gpu_only() {
        // in-flight case:
        let mut m = synthetic_manager(1000);
        m.synthetic.as_mut().unwrap().usage = 900;
        m.register_resource(rec(
            700,
            ResourcePriority::OptionalHistory,
            ContentSource::Regenerable,
        ))
        .unwrap();
        m.mark_used(ResourceId(1), 1, 0);
        m.on_submitted(1);

        for _ in 0..4 {
            let plan = m.sample_pressure().unwrap();
            assert!(plan.releases.is_empty(), "in-flight records are never released");
        }
        m.on_completed(1); // settles the serial
        let plan = m.sample_pressure().unwrap();
        assert_eq!(plan.releases.len(), 1);
    }

    #[test]
    fn locked_and_gpu_only_never_release() {
        let mut m = synthetic_manager(1000);
        m.synthetic.as_mut().unwrap().usage = 1300;
        m.register_resource(rec(
            500,
            ResourcePriority::OptionalHistory,
            ContentSource::CpuShadow,
        ))
        .unwrap();
        m.set_locked(ResourceId(1), true).unwrap();
        m.register_resource(rec(
            500,
            ResourcePriority::OptionalHistory,
            ContentSource::GpuOnly,
        ))
        .unwrap();

        // Three consecutive samples latch eviction; the latch + empty
        // candidate set is the ACTIONABLE-pressure shape (§16.11).
        let mut plan = eviction::EvictionPlan::default();
        for _ in 0..3 {
            plan = m.sample_pressure().unwrap();
        }
        assert!(plan.releases.is_empty(), "locked + unrestorable = nothing may release");
        assert!(m.metrics().registry_resident_bytes > 0, "nothing was freed");
        // Persistent pressure became a diagnostic (§16.11).
        assert_eq!(m.counters.persistent_pressure_events, 1);
    }

    #[test]
    fn required_resources_are_never_evicted() {
        let mut m = synthetic_manager(1000);
        m.synthetic.as_mut().unwrap().usage = 900;
        let _export = m
            .register_resource(rec(
                600,
                ResourcePriority::CurrentFrame,
                ContentSource::Regenerable,
            ))
            .unwrap();
        let _junk = m
            .register_resource(rec(
                80,
                ResourcePriority::OptionalHistory,
                ContentSource::Regenerable,
            ))
            .unwrap();

        let mut required = BTreeSet::new();
        required.insert(ResourceId(1)); // export pin (§16.10)
        let snap = m.query_local_memory().unwrap();
        let plan = m.plan_eviction(&snap, &required);
        let released: Vec<u64> = plan.releases.iter().map(|c| c.id.0).collect();
        assert_eq!(released, vec![2], "only the optional cache may release");
    }

    #[test]
    fn preflight_refuses_when_nothing_is_releasable() {
        let mut m = synthetic_manager(1000);
        m.synthetic.as_mut().unwrap().usage = 900;
        m.register_resource(rec(900, ResourcePriority::Critical, ContentSource::GpuOnly))
            .unwrap();

        let err = m.preflight_allocation(500, ResourcePriority::CurrentFrame).unwrap_err();
        match err {
            MemoryError::OutOfMemory { requested, shortfall, .. } => {
                assert_eq!(requested, 500);
                assert!(shortfall > 0);
            }
            other => panic!("expected OOM, got {other:?}"),
        }
        assert_eq!(m.counters.allocation_refusals, 1);
        assert_eq!(m.ledger.borrow().open_count(), 0, "no reservation leaked");
    }

    #[test]
    fn preflight_grants_and_permit_rollback_on_drop() {
        let mut m = synthetic_manager(1000);
        let permit = m.preflight_allocation(100, ResourcePriority::Cache).unwrap();
        assert_eq!(permit.bytes(), 100);
        assert_eq!(m.ledger.borrow().open_count(), 1);
        drop(permit);
        assert_eq!(
            m.ledger.borrow().open_count(),
            0,
            "drop releases the unused reservation"
        );
    }

    #[test]
    fn permit_commit_consumes_and_registration_follows() {
        let mut m = synthetic_manager(1000);
        let permit = m.preflight_allocation(100, ResourcePriority::Cache).unwrap();
        let _ = permit.commit();
        assert_eq!(m.ledger.borrow().open_count(), 0, "commit consumed the ledger entry");
        m.register_resource(rec(100, ResourcePriority::Cache, ContentSource::CpuShadow))
            .unwrap();
        m.complete_reload(ResourceId(1));
        assert_eq!(m.accounted_usage_approx(), 100);
    }

    #[test]
    fn fault_injection_fails_bounded_number_of_allocations() {
        let mut m = synthetic_manager(1000);
        m.synthetic.as_mut().unwrap().fail_next_allocations = 2;
        for _ in 0..2 {
            assert!(matches!(
                m.preflight_allocation(10, ResourcePriority::Cache),
                Err(MemoryError::OutOfMemory { .. })
            ));
        }
        assert!(m.preflight_allocation(10, ResourcePriority::Cache).is_ok());
        assert_eq!(m.counters.allocation_refusals, 2);
    }

    #[test]
    fn reload_queue_bounded_by_frame_budget() {
        let mut m = synthetic_manager(1 << 20);
        m.register_resource(rec(
            100,
            ResourcePriority::CurrentFrame,
            ContentSource::Disk(anim_core::id::AssetId::from_u128(1)),
        ))
        .unwrap();
        m.register_resource(rec(
            90,
            ResourcePriority::CurrentFrame,
            ContentSource::CpuShadow,
        ))
        .unwrap();
        m.release_resource(ResourceId(1));
        m.release_resource(ResourceId(2));

        let batch = m.reload_required(&[ResourceId(1), ResourceId(2)], 120).unwrap();
        assert_eq!(batch.scheduled.len(), 1);
        assert_eq!(batch.deferred.len(), 1);
        assert!(batch.backed_up);
        assert_eq!(m.counters.reload_stalls, 1);
        assert_eq!(m.registry[&ResourceId(1)].reload_state, ReloadState::Reloading);

        m.complete_reload(ResourceId(1));
        assert!(m.registry[&ResourceId(1)].resident);

        // Unknown required id is a typed error, not a silent skip.
        assert!(matches!(
            m.reload_required(&[ResourceId(99)], 100),
            Err(MemoryError::UnknownResource(ResourceId(99)))
        ));
    }

    #[test]
    fn device_loss_resets_fences_ledger_and_residency() {
        let mut m = synthetic_manager(1 << 20);
        m.register_resource(rec(100, ResourcePriority::Cache, ContentSource::CpuShadow))
            .unwrap();
        m.mark_used(ResourceId(1), 3, 2);
        m.on_submitted(3);
        m.on_completed(3);
        m.release_resource(ResourceId(1));

        let rebuilds = m.invalidate_for_device_loss();
        assert_eq!(rebuilds, 1);
        assert_eq!(m.fences.assign_next(), 1, "serial space restarted");
        assert_eq!(m.ledger.borrow().open_count(), 0);
        assert_eq!(m.registry[&ResourceId(1)].last_used_serial, 0);
        assert!(!m.registry[&ResourceId(1)].resident);
        assert!(!m.evicting);
        assert_eq!(m.over_budget_count, 0);
    }

    #[test]
    fn degrade_preview_drops_only_degradable() {
        let mut m = synthetic_manager(1 << 20);
        m.register_resource(rec(
            10,
            ResourcePriority::OptionalHistory,
            ContentSource::Regenerable,
        ))
        .unwrap();
        m.register_resource(rec(20, ResourcePriority::Cache, ContentSource::CpuShadow))
            .unwrap();
        m.register_resource(rec(
            30,
            ResourcePriority::CurrentFrame,
            ContentSource::Regenerable,
        ))
        .unwrap();
        m.set_locked(ResourceId(2), true).unwrap();

        let freed = m.degrade_preview();
        assert_eq!(freed, 1);
        assert!(!m.registry[&ResourceId(1)].resident);
        assert!(m.registry[&ResourceId(2)].resident);
        assert!(m.registry[&ResourceId(3)].resident);
    }

    #[test]
    fn metrics_generation_and_summary_are_diagnostics_shaped() {
        let mut m = synthetic_manager(1000);
        m.register_resource(rec(100, ResourcePriority::Cache, ContentSource::Regenerable))
            .unwrap();
        let _ = m.query_local_memory().unwrap();
        let s1 = m.metrics();
        assert_eq!(s1.generation, 1);
        assert!(s1.telemetry_available, "synthetic debug hook counts as available");
        assert_eq!(s1.registry_resident_bytes, 100);

        let _ = m.sample_pressure().unwrap();
        let s2 = m.metrics();
        assert_ne!(s1, s2);
        assert!(s2.summary_line().contains(&format!("gen={}", s2.generation)));
    }

    #[test]
    fn mark_used_unknown_id_is_silent_and_safe() {
        let mut m = synthetic_manager(1000);
        m.mark_used(ResourceId(999), 1, 1);
        assert!(m.registry.is_empty());
    }

    #[test]
    fn memory_error_display_carries_byte_context() {
        let e = MemoryError::OutOfMemory {
            requested: 500,
            budget: 800,
            effective_usage: 900,
            reservation_total: 50,
            shortfall: 150,
            detail: "test".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("requested 500"));
        assert!(msg.contains("over by 150"));
        assert!(msg.contains("soft target 800"));
    }

    // Accounted usage is private; tests use the metrics mirror.
    impl MemoryManager {
        fn accounted_usage_approx(&self) -> u64 {
            self.metrics().registry_resident_bytes
        }
    }
}
