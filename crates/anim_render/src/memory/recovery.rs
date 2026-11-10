//! OOM degradation and device-loss reconstruction policy (§16.3/§16.6).
//!
//! Preview degradation: previews may drop optional content (evict
//! OptionalHistory/Cache classes and skip ahead-of-time uploads) rather
//! than fail. Export: pinned requirements are NEVER silently dropped —
//! an OOME goes back to the caller with byte context (§16.11).
//!
//! Device loss: serial assumptions are invalid, so every residency
//! record flips to PendingReload with usage serials cleared; content
//! rebuilds from CPU/disk sources through the reload queue (§16.7 tail).

use super::resource::{ReloadState, ResourceId, ResourcePriority, ResourceRecord};
use std::collections::BTreeMap;

/// Preview-only degradation plan: which classes may be released, in
/// order (§16.6 "reduce preview-only optional targets").
pub(crate) fn preview_degradation_classes() -> [ResourcePriority; 2] {
    [ResourcePriority::OptionalHistory, ResourcePriority::Cache]
}

/// Is this record releaseable by preview degradation?
pub(crate) fn preview_degradable(rec: &ResourceRecord) -> bool {
    !rec.locked && preview_degradation_classes().contains(&rec.priority)
}

/// Apply device-loss invalidation to the registry (§16.7): every record
/// keeps identity + bytes + priority, but residency/reload/fence state
/// resets. Returns how many records the caller must now rebuild.
pub(crate) fn invalidate_for_device_loss(
    registry: &mut BTreeMap<ResourceId, ResourceRecord>,
) -> usize {
    let mut count = 0;
    for rec in registry.values_mut() {
        rec.resident = false;
        rec.last_used_serial = 0;
        rec.last_used_frame = 0;
        // Unrestorable GPU-only content is GONE with the device: such
        // records cannot reload; callers that need them must allocate
        // new ones. Keep the record (bytes still describe a resident
        // reality that no longer exists) — reload will type-error on
        // ContentSource::GpuOnly. Pinned behavior, §16.11 note.
        rec.reload_state = match rec.source {
            // Regenerate: re-run the generator of record (targets re-clear).
            super::resource::ContentSource::Regenerable
            | super::resource::ContentSource::Disk(_) => ReloadState::PendingReload,
            // CPU shadow copy exists: plain upload path.
            super::resource::ContentSource::CpuShadow => ReloadState::PendingReload,
            // Restorable GPU-only: rebuild via its restore path.
            super::resource::ContentSource::GpuOnlyWithRestore => {
                ReloadState::PendingReload
            }
            // Gone for good with the old device.
            super::resource::ContentSource::GpuOnly => ReloadState::Resident,
        };
        if rec.reload_state == ReloadState::PendingReload {
            count += 1;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::super::resource::{ContentSource, ReloadState, ResourceId};
    use super::*;

    fn rec(
        priority: ResourcePriority,
        source: ContentSource,
        locked: bool,
    ) -> ResourceRecord {
        ResourceRecord {
            id: ResourceId::PLACEHOLDER,
            label: "r".into(),
            bytes: 100,
            source,
            priority,
            last_used_serial: 7,
            last_used_frame: 3,
            resident: true,
            locked,
            reload_state: ReloadState::Resident,
        }
    }

    #[test]
    fn preview_degradation_covers_only_unlocked_low_classes() {
        assert!(preview_degradable(&rec(
            ResourcePriority::OptionalHistory,
            ContentSource::Regenerable,
            false
        )));
        assert!(preview_degradable(&rec(
            ResourcePriority::Cache,
            ContentSource::CpuShadow,
            false
        )));
        // Locked cache is untouchable (§16.6 never free locked).
        assert!(!preview_degradable(&rec(
            ResourcePriority::Cache,
            ContentSource::CpuShadow,
            true
        )));
        // CurrentFrame content is never degradation fodder.
        assert!(!preview_degradable(&rec(
            ResourcePriority::CurrentFrame,
            ContentSource::Regenerable,
            false
        )));
        assert!(!preview_degradable(&rec(
            ResourcePriority::Critical,
            ContentSource::Regenerable,
            false
        )));
    }

    #[test]
    fn device_loss_flips_records_and_counts_rebuildables() {
        let mut registry = BTreeMap::new();
        registry.insert(
            ResourceId(1),
            rec(
                ResourcePriority::Cache,
                ContentSource::Disk(anim_core::id::AssetId::from_u128(1)),
                false,
            ),
        );
        registry.insert(
            ResourceId(2),
            rec(ResourcePriority::CurrentFrame, ContentSource::CpuShadow, false),
        );
        registry.insert(
            ResourceId(3),
            rec(ResourcePriority::NextFrame, ContentSource::Regenerable, false),
        );
        registry.insert(
            ResourceId(4),
            rec(ResourcePriority::NextFrame, ContentSource::GpuOnly, false),
        );

        let rebuilds = invalidate_for_device_loss(&mut registry);
        // 3 rebuildable; the pure-GpuOnly record is unrecoverable.
        assert_eq!(rebuilds, 3);

        assert!(!registry[&ResourceId(1)].resident);
        assert_eq!(registry[&ResourceId(1)].reload_state, ReloadState::PendingReload);
        assert_eq!(registry[&ResourceId(1)].last_used_serial, 0);
        // GpuOnly: resident flag down but state stays Resident = dead.
        assert_eq!(registry[&ResourceId(4)].reload_state, ReloadState::Resident);
        assert!(!registry[&ResourceId(4)].resident);
    }

    #[test]
    fn degradation_class_order_is_optional_first() {
        let classes = preview_degradation_classes();
        assert_eq!(classes, [ResourcePriority::OptionalHistory, ResourcePriority::Cache]);
    }
}
