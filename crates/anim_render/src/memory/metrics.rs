//! Immutable usage, pressure, eviction and stall metrics (§16.3).
//!
//! Every snapshot is an immutable COPY (§16.8 last paragraph): callers
//! can never observe torn reads of manager internals, and metrics can
//! feed diagnostics (anim_headless `budget_report`, WP07+ UI) without
//! revealing DXGI pointers (§16.4).

use super::eviction::BudgetSnapshot;

/// High-water counters accumulated by the manager over its lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MetricCounters {
    /// Successful `preflight_allocation` grants (incl. post-eviction).
    pub allocations_granted: u64,
    /// Typed OOM refusals (never retried past the bounded policy).
    pub allocation_refusals: u64,
    /// Resources actually released by eviction plans that were APPLIED.
    pub evictions: u64,
    /// Bytes released by those evictions.
    pub evicted_bytes: u64,
    /// Reload batches handed out.
    pub reload_batches: u64,
    /// Frames where a REQUIRED entry could not be scheduled (stalls).
    pub reload_stalls: u64,
    /// DXGI samples taken (0 = telemetry never available).
    pub telemetry_samples: u64,
    /// Failed telemetry queries (§16.11 telemetry is advisory).
    pub telemetry_errors: u64,
    /// Consecutive-pressure diagnostic events raised (§16.11).
    pub persistent_pressure_events: u64,
}

/// Immutable point-in-time diagnostic copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricsSnapshot {
    /// Monotonic snapshot sequence (generation of the metrics view).
    pub generation: u64,
    pub counters: MetricCounters,
    /// Last budget view the manager computed (may be estimates-only).
    pub last_snapshot: Option<BudgetSnapshot>,
    /// Telemetry availability flag (§16.11 "mark telemetry unavailable").
    pub telemetry_available: bool,
    /// Sanity invariant: accounted bytes can never exceed the effective
    /// usage HISTORY — surfaced for diagnostics, not proof of foreign
    /// ownership (§16.7 differences are diagnostics only).
    pub registry_resident_bytes: u64,
}

impl MetricsSnapshot {
    /// One-line JSON-ready summary for the headless report (no DXGI
    /// pointers; stable field order for canonical output).
    pub fn summary_line(&self) -> String {
        let c = &self.counters;
        format!(
            "gen={} budget={} effective_usage={} accounted={} resident_registry={} tele={} alloc_ok={} alloc_oom={} evict={} evict_bytes={} reload_batches={} stalls={} pressure={}",
            self.generation,
            self.last_snapshot.map(|s| s.budget).unwrap_or(0),
            self.last_snapshot.map(|s| s.effective_usage).unwrap_or(0),
            self.last_snapshot.map(|s| s.accounted_usage).unwrap_or(0),
            self.registry_resident_bytes,
            self.telemetry_available,
            c.allocations_granted,
            c.allocation_refusals,
            c.evictions,
            c.evicted_bytes,
            c.reload_batches,
            c.reload_stalls,
            c.persistent_pressure_events,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_line_is_stable_and_pointer_free() {
        let snap = MetricsSnapshot {
            generation: 3,
            counters: MetricCounters {
                allocations_granted: 2,
                allocation_refusals: 1,
                evictions: 4,
                evicted_bytes: 4096,
                reload_batches: 5,
                reload_stalls: 1,
                telemetry_samples: 9,
                telemetry_errors: 0,
                persistent_pressure_events: 0,
            },
            last_snapshot: None,
            telemetry_available: false,
            registry_resident_bytes: 123,
        };
        let line = snap.summary_line();
        assert!(line.starts_with("gen=3 budget=0 "));
        assert!(line.contains("accounted=0"));
        assert!(line.contains("evict_bytes=4096"));
        assert!(line.contains("stalls=1"));
        assert!(!line.contains("ptr"), "DXGI pointers must never serialize");
    }

    #[test]
    fn counters_default_to_zero() {
        assert_eq!(MetricCounters::default().evictions, 0);
        assert_eq!(MetricCounters::default().telemetry_samples, 0);
    }
}
