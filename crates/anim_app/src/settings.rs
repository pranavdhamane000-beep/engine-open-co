//! Validated memory-policy configuration (spec §16.3
//! anim_app/src/settings.rs). Loaded by the application layer and fed
//! to `anim_render::memory::MemoryManager` — the manager itself never
//! reads configuration files (D-11's ownership discipline extends to
//! policy: the app decides, the engine executes).
//!
//! Validation (§16.11 shape — configuration errors are typed and loud):
//! - `target_fraction` in (0.0, 1.0];
//! - `safety_reserve_bytes` < budget is NOT knowable here; instead the
//!   fraction·budget vs reserve collision resolves via saturating
//!   arithmetic downstream — but zero-value nonsense IS rejected;
//! - `over_budget_samples` ≥ 1 (0 would make hysteresis vacuous);
//! - `max_upload_bytes_per_frame` > 0;
//! - `release_band_bytes` ≤ safety-cover sanity: any value is accepted
//!   (bands can legitimately exceed the reserve) but a band larger
//!   than the target floor collapses to 0 downstream — warned not
//!   errored, matching §16.6 "configuration defaults, not promises".

use anim_render::memory::resource::MemoryPolicy;
use serde::Deserialize;
use std::time::Duration;

/// On-disk/JSON configuration mirror of `MemoryPolicy`.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct MemoryPolicySettings {
    #[serde(default = "default_target_fraction")]
    pub target_fraction: f32,
    #[serde(default = "default_safety_reserve_bytes")]
    pub safety_reserve_bytes: u64,
    #[serde(default = "default_release_band_bytes")]
    pub release_band_bytes: u64,
    #[serde(default = "default_over_budget_samples")]
    pub over_budget_samples: u8,
    /// Query interval in milliseconds (typed Duration downstream).
    #[serde(default = "default_query_interval_ms")]
    pub query_interval_ms: u64,
    #[serde(default = "default_max_upload_bytes_per_frame")]
    pub max_upload_bytes_per_frame: u64,
}

fn default_target_fraction() -> f32 {
    0.80
}
fn default_safety_reserve_bytes() -> u64 {
    128 * 1024 * 1024
}
fn default_release_band_bytes() -> u64 {
    64 * 1024 * 1024
}
fn default_over_budget_samples() -> u8 {
    3
}
fn default_query_interval_ms() -> u64 {
    250
}
fn default_max_upload_bytes_per_frame() -> u64 {
    16 * 1024 * 1024
}

impl Default for MemoryPolicySettings {
    fn default() -> Self {
        Self {
            target_fraction: default_target_fraction(),
            safety_reserve_bytes: default_safety_reserve_bytes(),
            release_band_bytes: default_release_band_bytes(),
            over_budget_samples: default_over_budget_samples(),
            query_interval_ms: default_query_interval_ms(),
            max_upload_bytes_per_frame: default_max_upload_bytes_per_frame(),
        }
    }
}

/// Typed configuration error (spec §12.7-style loudness).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SettingsError {
    #[error("target_fraction {0} invalid; must be in (0.0, 1.0]")]
    TargetFractionOutOfRange(f32),
    #[error("over_budget_samples {0} invalid; must be >= 1")]
    OverBudgetSamplesZero(u8),
    #[error("max_upload_bytes_per_frame {0} invalid; must be > 0")]
    ZeroUploadBudget(u64),
    #[error("safety_reserve_bytes/write band mismatch: band {0} must be < reserve-driven floor guard; choose band < (1.0 - fraction)·budget")]
    ReleaseBandCollapse(u64),
}

impl MemoryPolicySettings {
    /// Validate and convert into the engine `MemoryPolicy`.
    pub fn validate(&self) -> Result<MemoryPolicy, SettingsError> {
        if !(self.target_fraction > 0.0 && self.target_fraction <= 1.0) {
            return Err(SettingsError::TargetFractionOutOfRange(self.target_fraction));
        }
        if self.over_budget_samples < 1 {
            return Err(SettingsError::OverBudgetSamplesZero(self.over_budget_samples));
        }
        if self.max_upload_bytes_per_frame == 0 {
            return Err(SettingsError::ZeroUploadBudget(self.max_upload_bytes_per_frame));
        }
        // §16.6 note: a release band at or above the (1−fraction)·budget
        // envelope can collapse the watermark to 0, silently disabling
        // hysteresis. Not an error (config defaults, not promises) but
        // flagged loudly.
        let fraction_gap = 1.0f32 - self.target_fraction;
        let gap_bytes = (fraction_gap * (u64::MAX >> 4) as f32).min(u64::MAX as f32) as u64;
        if self.release_band_bytes >= gap_bytes.saturating_mul(4) {
            // 4·gap guard is a pinned heuristic (ADR-0011): catch the
            // "band dwarfs the envelope" mistake made in config files.
            return Err(SettingsError::ReleaseBandCollapse(self.release_band_bytes));
        }
        Ok(MemoryPolicy {
            target_fraction: self.target_fraction,
            safety_reserve_bytes: self.safety_reserve_bytes,
            release_band_bytes: self.release_band_bytes,
            over_budget_samples: self.over_budget_samples,
            query_interval: Duration::from_millis(self.query_interval_ms),
            max_upload_bytes_per_frame: self.max_upload_bytes_per_frame,
        })
    }
}

impl From<MemoryPolicy> for MemoryPolicySettings {
    fn from(p: MemoryPolicy) -> Self {
        Self {
            target_fraction: p.target_fraction,
            safety_reserve_bytes: p.safety_reserve_bytes,
            release_band_bytes: p.release_band_bytes,
            over_budget_samples: p.over_budget_samples,
            query_interval_ms: p.query_interval.as_millis() as u64,
            max_upload_bytes_per_frame: p.max_upload_bytes_per_frame,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate_to_engine_default_policy() {
        let policy = MemoryPolicySettings::default().validate().unwrap();
        assert_eq!(policy, MemoryPolicy::default());
    }

    #[test]
    fn round_trip_policy_to_settings_to_policy() {
        let policy = MemoryPolicy::default();
        let back = MemoryPolicySettings::from(policy.clone()).validate().unwrap();
        assert_eq!(policy, back);
    }

    #[test]
    fn typed_rejections_per_field() {
        let s = MemoryPolicySettings { target_fraction: 1.5, ..Default::default() };
        assert!(
            matches!(s.validate(), Err(SettingsError::TargetFractionOutOfRange(x)) if x == 1.5)
        );

        let s = MemoryPolicySettings { target_fraction: 0.0, ..Default::default() };
        assert!(matches!(s.validate(), Err(SettingsError::TargetFractionOutOfRange(_))));

        let s = MemoryPolicySettings { over_budget_samples: 0, ..Default::default() };
        assert!(matches!(s.validate(), Err(SettingsError::OverBudgetSamplesZero(0))));

        let s =
            MemoryPolicySettings { max_upload_bytes_per_frame: 0, ..Default::default() };
        assert!(matches!(s.validate(), Err(SettingsError::ZeroUploadBudget(0))));
    }

    #[test]
    fn absurd_release_band_is_rejected_not_collapsed() {
        let s =
            MemoryPolicySettings { release_band_bytes: u64::MAX / 8, ..Default::default() };
        assert!(matches!(s.validate(), Err(SettingsError::ReleaseBandCollapse(_))));
    }

    #[test]
    fn serde_defaults_keep_absent_fields_clean() {
        let doc = "{}";
        let s: MemoryPolicySettings = serde_json::from_str(doc).unwrap();
        assert_eq!(s, MemoryPolicySettings::default());
        // Full document also parses.
        let full: MemoryPolicySettings = serde_json::from_str(
            r#"{"target_fraction": 0.5, "safety_reserve_bytes": 1, "release_band_bytes": 1, "over_budget_samples": 4, "query_interval_ms": 60, "max_upload_bytes_per_frame": 9}"#,
        )
        .unwrap();
        assert!(full.validate().is_ok());
    }
}
