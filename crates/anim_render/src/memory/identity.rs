//! Adapter identity + residency policy (spec §16.3 identity.rs; real
//! DXGI correlation lives in `dxgi.rs`, chunk 2 — this module owns the
//! PURE identity state machine the rest of the manager reasons about).
//!
//! §16.6 rule (pinned): query only the adapter correlated with the
//! rendering device. If identity cannot be established confidently:
//! continue with internal estimates, use a conservative target, and
//! mark telemetry unavailable. NEVER query an unrelated adapter.
//!
//! Correlation strategy (ADR-0011, from ADR-0003 probe knowledge):
//! 1. `Hints::luid` — an explicit 64-bit LUID the CALLER obtained for
//!    the rendering device (probe-style paths). All-64-bit match +
//!    vendor/device cross-check, never a substitution.
//! 2. Unique vendor+device+description match against DXGI enumeration.
//! 3. Nothing confident → `Unavailable(reason)` telemetry state.

/// Spec §16.8 `AdapterIdentity` (no DXGI pointers exposed, §16.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterIdentity {
    pub luid: i64,
    pub description: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub dedicated_video_memory: u64,
}

/// Caller-provided correlation hints (respects D-11: the ENGINE never
/// picks an adapter; it only correlates the one already in use).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdentityHints {
    /// LUID for the rendering device, when the caller can prove it.
    pub luid: Option<i64>,
    /// wgpu `AdapterInfo`-derived ids (from the chosen adapter).
    pub vendor_id: Option<u32>,
    pub device_id: Option<u32>,
    pub description: Option<String>,
}

/// Identity state machine (pure; dxgi.rs fills Resolved).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityState {
    Unprobed,
    Resolved(AdapterIdentity),
    /// Deliberate state per §16.6: telemetry stays OFF, internal
    /// accounting is authoritative, target forced conservative.
    Unavailable {
        reason: String,
    },
}

impl IdentityState {
    /// Does this state permit real DXGI queries?
    pub fn telemetry_allowed(&self) -> bool {
        matches!(self, Self::Resolved(_))
    }

    /// §16.6 conservative fallback: when telemetry is unavailable the
    /// effective target fraction is clamped down (e.g. 0.60) instead of
    /// the configured 0.80 — budget overestimation is the dangerous
    /// direction, underestimation is merely conservative.
    pub fn effective_target_fraction(policy_fraction: f32, state: &IdentityState) -> f32 {
        if state.telemetry_allowed() {
            policy_fraction
        } else {
            policy_fraction.min(defaults::CONSERVATIVE_TARGET_FRACTION)
        }
    }
}

/// Pure scoring of one DXGI-enumerated adapter against the hints.
/// `Some(identity)` only when the match is CONFIDENT:
/// - With an explicit LUID hint: all-64-bit LUID equality AND (when the
///   hint carries them) vendor+device agreement. One mismatch → None,
///   NEVER a best-effort pick (ADR-0003 negative test lesson).
/// - Without a LUID: unique match on (vendor, device, description
///   prefix-insensitive); ambiguity (>1) → None.
pub fn correlate(
    hints: &IdentityHints,
    candidates: &[AdapterIdentity],
) -> Option<AdapterIdentity> {
    // LUID is the strongest key when supplied.
    if let Some(luid) = hints.luid {
        let by_luid: Vec<&AdapterIdentity> =
            candidates.iter().filter(|c| c.luid == luid).collect();
        if by_luid.len() != 1 {
            return None; // zero or ambiguous: never substitute
        }
        let found = by_luid[0];
        // Cross-check ids when the caller supplied them (ADR-0003).
        if let (Some(v), Some(d)) = (hints.vendor_id, hints.device_id) {
            if found.vendor_id != v || found.device_id != d {
                return None; // ids disagree → identity NOT confident
            }
        }
        return Some(found.clone());
    }

    // Fall back: unique (vendor, device, description-substring) match.
    let ids_match = |c: &AdapterIdentity| {
        let id_hit = match (hints.vendor_id, hints.device_id) {
            (Some(v), Some(d)) => c.vendor_id == v && c.device_id == d,
            (Some(v), None) => c.vendor_id == v,
            (None, Some(d)) => c.device_id == d,
            (None, None) => true, // description-only matching
        };
        let desc_hit = hints
            .description
            .as_ref()
            .map(|d| fuzzy_contains(&c.description, d))
            .unwrap_or(true);
        id_hit && desc_hit
    };
    let matches: Vec<&AdapterIdentity> =
        candidates.iter().filter(|c| ids_match(c)).collect();
    if matches.len() == 1 {
        return Some(matches[0].clone());
    }
    None
}

/// Case-insensitive whitespace-tolerant containment (DXGI descriptions
/// like "NVIDIA GeForce RTX 3050" vs wgpu "NVIDIA GeForce RTX 3050").
fn fuzzy_contains(haystack: &str, needle: &str) -> bool {
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    let (h, n) = (norm(haystack), norm(needle));
    h.contains(&n) || n.contains(&h)
}

/// Pinned conservative constant (ADR-0011; a change = golden event).
pub(crate) mod defaults {
    /// Target fraction when telemetry is UNAVAILABLE (lower than 0.80,
    /// §16.6 conservative fallback direction).
    pub const CONSERVATIVE_TARGET_FRACTION: f32 = 0.60;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dxgi(luid: i64, vendor: u32, device: u32, desc: &str, mem: u64) -> AdapterIdentity {
        AdapterIdentity {
            luid,
            description: desc.into(),
            vendor_id: vendor,
            device_id: device,
            dedicated_video_memory: mem,
        }
    }

    fn candidates() -> Vec<AdapterIdentity> {
        vec![
            dxgi(
                0x11AA_22BB_33CC_44DD,
                0x10DE,
                0x2507,
                "NVIDIA GeForce RTX 3050",
                4_294_967_296,
            ),
            dxgi(0x0000_0000_0001_1234, 0x8086, 0x9A49, "Intel(R) UHD Graphics", 0),
        ]
    }

    #[test]
    fn luid_hint_correlates_with_cross_check() {
        let hints = IdentityHints {
            luid: Some(0x11AA_22BB_33CC_44DD),
            vendor_id: Some(0x10DE),
            device_id: Some(0x2507),
            description: None,
        };
        let found =
            correlate(&hints, &candidates()).expect("exact LUID + ids must resolve");
        assert_eq!(found.description, "NVIDIA GeForce RTX 3050");
        assert!(found.dedicated_video_memory > 0);
    }

    #[test]
    fn luid_with_disagreeing_ids_is_not_confident() {
        let hints = IdentityHints {
            luid: Some(0x11AA_22BB_33CC_44DD),
            vendor_id: Some(0x8086), // WRONGVENDOR on purpose
            device_id: Some(0x2507),
            description: None,
        };
        assert!(
            correlate(&hints, &candidates()).is_none(),
            "never substitute across id disagreement"
        );
    }

    #[test]
    fn luid_matching_no_adapter_is_unavailable_never_substituted() {
        let hints = IdentityHints {
            luid: Some(0x0000_0000_DEAD_0001), // probe-mismatch style
            vendor_id: None,
            device_id: None,
            description: None,
        };
        assert!(correlate(&hints, &candidates()).is_none());
    }

    #[test]
    fn ambiguous_unix_style_matching_without_luid_fails() {
        // Two identical vendor/device entries → not unique → None.
        let two = vec![
            dxgi(1, 0x10DE, 0x2507, "NVIDIA GeForce RTX 3050", 1),
            dxgi(2, 0x10DE, 0x2507, "NVIDIA GeForce RTX 3050 (TAO)", 1),
        ];
        let hints = IdentityHints {
            vendor_id: Some(0x10DE),
            device_id: Some(0x2507),
            ..Default::default()
        };
        assert!(correlate(&hints, &two).is_none(), "ambiguity is not confidence");
    }

    #[test]
    fn unique_description_match_works_with_whitespace_variants() {
        let hints = IdentityHints {
            vendor_id: None,
            device_id: None,
            description: Some("geforce rtx 3050".into()),
            ..Default::default()
        };
        let found = correlate(&hints, &candidates()).expect("unique description match");
        assert_eq!(found.vendor_id, 0x10DE);
    }

    #[test]
    fn conservative_fraction_applies_only_without_telemetry() {
        let resolved = IdentityState::Resolved(dxgi(9, 1, 2, "x", 3));
        assert_eq!(
            IdentityState::effective_target_fraction(0.80, &resolved),
            0.80,
            "resolved telemetry keeps the configured fraction"
        );
        let unavail = IdentityState::Unavailable { reason: "no LUID".into() };
        assert!(!unavail.telemetry_allowed());
        assert_eq!(
            IdentityState::effective_target_fraction(0.80, &unavail),
            0.60,
            "conservative fallback fraction (§16.6)"
        );
    }
}
