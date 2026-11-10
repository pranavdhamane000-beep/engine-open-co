//! Resource registry types (spec §16.8).
//!
//! Every owned texture/buffer is registered BEFORE its GPU handle is
//! exposed to passes (§16.8: "Records are created before GPU handles are
//! exposed to passes. Access uses registry IDs."). `ResourceId` is
//! assigned by the registry monotonically — the caller-supplied
//! `record.id` is a placeholder and is OVERWRITTEN on registration
//! (pinned: `register_resource` returns the authoritative id).
//!
//! Records mutate ONLY through MemoryManager methods (allocation,
//! release, use marking, locking, fence completion, reload transition);
//! mutation on drop of an `AllocationPermit` releases its reservation.

use std::time::Duration;

/// Registry-assigned resource identity (monotonic u64; stable ordering
/// key for eviction tie-breaks per §16.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceId(pub u64);

impl ResourceId {
    /// Placeholder the CALLER may leave in a submitted record; the
    /// registry overwrites it. Never resolves to a real resource.
    pub const PLACEHOLDER: ResourceId = ResourceId(0);
}

/// Where content comes from — the reload-cost class (§16.7 prefers
/// disk-backed/regenerable first, then CPU-shadowed).
#[derive(Debug, Clone, PartialEq)]
pub enum ContentSource {
    /// Content-addressed asset object on disk (reload = re-decode/upload).
    Disk(anim_core::id::AssetId),
    /// Reconstructible by re-execution (e.g. WP06 internal render targets
    /// re-cleared/re-rendered); no CPU copy retained.
    Regenerable,
    /// A CPU-side copy exists (reload = plain upload, no re-decode).
    CpuShadow,
    /// GPU-only and NOT restorable — never selected for eviction and
    /// never released while resident (spec: "restore path required").
    GpuOnlyWithRestore,
    /// GPU-only with NO restore path — permanently excluded from all
    /// eviction and from device-loss reconstruction.
    GpuOnly,
}

/// Residency priority table (§16.8). Eviction candidacy ranks LEAST
/// valuable first: OptionalHistory dies before Critical ever does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourcePriority {
    /// Pin-alike: never evicted while resident (export active-frame
    /// pins land here).
    Critical,
    /// In use by the frame being composed this instant.
    CurrentFrame,
    /// Anticipated for the next frame (upload-ahead window).
    NextFrame,
    /// Reusable cache (asset texture caches).
    Cache,
    /// History the editor preview can drop silently.
    OptionalHistory,
}

/// Reload pipeline state machine (§16.8 `reload_state`; values pinned).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadState {
    /// Record born not-yet-resident; waiting on a reload batch.
    PendingReload,
    /// A reload batch has claimed it (bounded queue slot).
    Reloading,
    /// GPU content live; resident == true.
    Resident,
}

/// One registry entry (spec §16.8 struct, field-for-field).
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceRecord {
    /// OVERWRITTEN by the registry at registration; keep placeholder.
    pub id: ResourceId,
    /// Pass-visible debugging label (never a DXGI pointer, §16.4).
    pub label: String,
    /// Estimated committed bytes (this record's authoritative size).
    pub bytes: u64,
    pub source: ContentSource,
    pub priority: ResourcePriority,
    /// Highest submission serial that referenced this resource.
    pub last_used_serial: u64,
    pub last_used_frame: u64,
    pub resident: bool,
    pub locked: bool,
    pub reload_state: ReloadState,
}

impl ResourceRecord {
    /// Validation enforced at registration (§16.6 conservative spirit):
    /// zero-byte records and unlabeled records are configuration errors.
    pub fn validate(&self) -> Result<(), String> {
        if self.bytes == 0 {
            return Err(format!("record {:?}: bytes must be > 0", self.label));
        }
        if self.label.trim().is_empty() {
            return Err("record label must be non-empty".into());
        }
        Ok(())
    }
}

/// Validated memory policy (§16.8 `MemoryPolicy`). Defaults are §16.6
/// configuration defaults, NOT hardware promises.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryPolicy {
    /// Initial target fraction of the observed budget (0.80 default).
    pub target_fraction: f32,
    /// Absolute reserve subtracted from the budget (saturating).
    pub safety_reserve_bytes: u64,
    /// Lower release watermark distance below the soft target; eviction
    /// CONTINUES until projected usage < soft_target - release_band.
    pub release_band_bytes: u64,
    /// Consecutive over-budget samples before eviction engages.
    pub over_budget_samples: u8,
    /// Minimum interval between real telemetry queries.
    pub query_interval: Duration,
    /// Upload throttle per frame for bounded streaming.
    pub max_upload_bytes_per_frame: u64,
}

pub(crate) mod default_policy {
    use super::MemoryPolicy;
    use std::time::Duration;

    /// §16.6 defaults (pinned; a change here is a golden event).
    pub const TARGET_FRACTION: f32 = 0.80;
    pub const SAFETY_RESERVE_BYTES: u64 = 128 * 1024 * 1024;
    pub const RELEASE_BAND_BYTES: u64 = 64 * 1024 * 1024;
    pub const OVER_BUDGET_SAMPLES: u8 = 3;
    pub const QUERY_INTERVAL: Duration = Duration::from_millis(250);
    pub const MAX_UPLOAD_BYTES_PER_FRAME: u64 = 16 * 1024 * 1024;

    pub fn policy() -> MemoryPolicy {
        MemoryPolicy {
            target_fraction: TARGET_FRACTION,
            safety_reserve_bytes: SAFETY_RESERVE_BYTES,
            release_band_bytes: RELEASE_BAND_BYTES,
            over_budget_samples: OVER_BUDGET_SAMPLES,
            query_interval: QUERY_INTERVAL,
            max_upload_bytes_per_frame: MAX_UPLOAD_BYTES_PER_FRAME,
        }
    }
}

impl Default for MemoryPolicy {
    fn default() -> Self {
        default_policy::policy()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(label: &str, bytes: u64) -> ResourceRecord {
        ResourceRecord {
            id: ResourceId::PLACEHOLDER,
            label: label.into(),
            bytes,
            source: ContentSource::Regenerable,
            priority: ResourcePriority::CurrentFrame,
            last_used_serial: 0,
            last_used_frame: 0,
            resident: true,
            locked: false,
            reload_state: ReloadState::Resident,
        }
    }

    #[test]
    fn validation_rejects_zero_bytes_and_blank_labels() {
        assert!(record("ok", 1024).validate().is_ok());
        // zero bytes rejected
        assert!(record("bad", 0).validate().is_err());
        // blank label rejected
        let mut blank = record(" \t ", 1);
        blank.label = " \t ".into();
        assert!(blank.validate().is_err());
    }

    #[test]
    fn policy_defaults_match_the_pinned_table() {
        let p = MemoryPolicy::default();
        assert!((p.target_fraction - 0.80).abs() < 1e-6);
        assert_eq!(p.safety_reserve_bytes, 128 * 1024 * 1024);
        assert_eq!(p.release_band_bytes, 64 * 1024 * 1024);
        assert_eq!(p.over_budget_samples, 3);
        assert_eq!(p.query_interval, Duration::from_millis(250));
        assert_eq!(p.max_upload_bytes_per_frame, 16 * 1024 * 1024);
    }

    #[test]
    fn priority_ordering_is_least_valuable_first() {
        // Natural Ord follows the §16.8 declaration order (Critical first
        // = most valuable numerically LOWEST). Eviction never relies on
        // this: it uses the explicit `priority_rank_for_eviction` map,
        // pinned here alongside.
        let mut v = vec![
            ResourcePriority::CurrentFrame,
            ResourcePriority::OptionalHistory,
            ResourcePriority::Critical,
            ResourcePriority::NextFrame,
            ResourcePriority::Cache,
        ];
        v.sort();
        assert_eq!(
            v,
            vec![
                ResourcePriority::Critical,
                ResourcePriority::CurrentFrame,
                ResourcePriority::NextFrame,
                ResourcePriority::Cache,
                ResourcePriority::OptionalHistory,
            ]
        );

        // EVICTION rank (least valuable numerically first = OptionalHistory 0):
        assert_eq!(rank(&ResourcePriority::OptionalHistory), 0);
        assert_eq!(rank(&ResourcePriority::Cache), 1);
        assert_eq!(rank(&ResourcePriority::NextFrame), 2);
        assert_eq!(rank(&ResourcePriority::CurrentFrame), 3);
        assert_eq!(rank(&ResourcePriority::Critical), 4);
    }

    // Cross-crate-visible for this assertion set (eviction's rank map).
    use crate::memory::eviction::priority_rank_for_eviction as rank;
}
