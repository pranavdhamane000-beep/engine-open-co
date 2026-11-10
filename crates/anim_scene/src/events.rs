//! Event extraction, occurrence IDs and canonical sorting (spec §13.7).
//!
//! * occurrence index = the index of the time in `EventDefinition::times`
//!   (a STABLE source key per §13.7 — never hash-map/filesystem order)
//! * `EventOccurrenceId` = BLAKE3("anim_engine/event_occurrence_v1" over
//!   EventDefinitionId bytes + occurrence index), a DETERMINISTIC
//!   derivation from (EventDefinitionId, occurrence_index)
//! * `local_sequence` = occurrence_index
//! * sort key: (event_time, priority, source_node_id, local_sequence,
//!   occurrence_id); UUID comparison uses its 16 bytes
//! * frame ownership: frame n owns events with
//!   containing-frame == n ⟺ time_of(n) ≤ t < time_of(n+1); an event
//!   exactly AT the upper boundary belongs to the NEXT frame

use crate::error::{SceneError, SceneResult};
use crate::snapshot::SceneEvent;
use anim_core::{EventDefinitionId, EventOccurrenceId, FrameRate};
use anim_project::{EventDefinition, ProjectDocument, SceneData};

use anim_assets::hash::StreamingHasher;

/// Deterministic occurrence id (ADR-0008): BLAKE3-based derivation.
pub fn occurrence_id(
    def_id: &EventDefinitionId,
    occurrence_index: u32,
) -> EventOccurrenceId {
    let mut h = StreamingHasher::new();
    h.update(b"anim_engine/event_occurrence_v1");
    h.update(def_id.0.as_bytes());
    let mut buf = [0u8; 4];
    buf.copy_from_slice(&occurrence_index.to_le_bytes());
    h.update(&buf);
    let digest = h.finalize().0;
    let id16: [u8; 16] = digest[..16].try_into().expect("prefix");
    EventOccurrenceId::from_u128(u128::from_le_bytes(id16))
}

/// All occurrences of every definition whose containing frame == `frame`,
/// in canonical order. `events_for_frame` returns this as an Arc slice.
pub(crate) fn occurrences_in_frame(
    scene: &SceneData,
    fps: FrameRate,
    frame: i64,
) -> SceneResult<Vec<SceneEvent>> {
    let mut out: Vec<SceneEvent> = Vec::new();
    for (def_id, def) in &scene.events {
        validate_definition(def)?;
        for (idx, t) in def.times.iter().enumerate() {
            let usize_idx = u32::try_from(idx).map_err(|_| {
                SceneError::EventOrderOverflow(format!(
                    "definition {def_id}: occurrence index {idx} exceeds u32"
                ))
            })?;
            let containing = fps.frame_containing(t.clone())?;
            if containing.frame() == frame {
                let event_time = containing; // event time STAMPED at its frame
                out.push(SceneEvent {
                    occurrence_id: occurrence_id(def_id, usize_idx),
                    time: event_time,
                    priority: def.priority,
                    source_id: def.source,
                    local_sequence: usize_idx,
                    kind: def.kind,
                });
            }
        }
    }
    sort_canonical(&mut out);
    Ok(out)
}

fn validate_definition(def: &EventDefinition) -> SceneResult<()> {
    // structural re-checks at evaluation time (document validation should
    // have caught these; never evaluate a malformed definition silently)
    for w in def.times.windows(2) {
        if w[0] >= w[1] {
            return Err(SceneError::InvalidPropertyKeys(format!(
                "event definition '{}': occurrence times not strictly increasing",
                def.name
            )));
        }
    }
    Ok(())
}

/// CANONICAL comparator per §13.7: (event_time, priority, source_node_id,
/// local_sequence, occurrence_id). UUID fields compare their 16 bytes
/// (uuid crate Ord is bytewise; typed IDs derive it).
pub(crate) fn sort_canonical(events: &mut [SceneEvent]) {
    events.sort_by(|a, b| {
        a.time
            .cmp(&b.time)
            .then(a.priority.cmp(&b.priority))
            .then(a.source_id.cmp(&b.source_id))
            .then(a.local_sequence.cmp(&b.local_sequence))
            .then(a.occurrence_id.cmp(&b.occurrence_id))
    });
}

/// Scene event for one whole frame (public spec §13.9 API lives in
/// evaluator.rs; this is the shared implementation).
pub(crate) fn events_for_frame_impl(
    document: &ProjectDocument,
    scene_id: anim_core::SceneId,
    frame: i64,
) -> SceneResult<Vec<SceneEvent>> {
    let scene = document
        .scenes
        .get(&scene_id)
        .ok_or_else(|| SceneError::MissingScene(scene_id.to_string()))?;
    occurrences_in_frame(scene, document.frame_rate, frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::SceneEvent;
    use anim_core::{FramePos, Rational};
    use anim_project::EventKind;

    /// Canonical event builders with pinned distinct fields.
    fn ev(id: u128, frame: i64, priority: i32, sequence: u32) -> SceneEvent {
        SceneEvent {
            occurrence_id: EventOccurrenceId::from_u128(id),
            time: FramePos::new(frame, Rational::zero()).unwrap(),
            priority,
            source_id: anim_core::NodeId::from_u128(id * 10),
            local_sequence: sequence,
            kind: EventKind::Spawn,
        }
    }

    #[test]
    fn occurrence_id_is_deterministic_and_injective() {
        let def_a = EventDefinitionId::from_u128(42);
        let def_b = EventDefinitionId::from_u128(43);
        // same args → same id (the two-process requirement)
        assert_eq!(occurrence_id(&def_a, 0), occurrence_id(&def_a, 0));
        // different occurrence index → different id
        assert_ne!(occurrence_id(&def_a, 0), occurrence_id(&def_a, 1));
        // different definition → different id even for the same index
        assert_ne!(occurrence_id(&def_a, 0), occurrence_id(&def_b, 0));
    }

    #[test]
    fn canonical_order_is_insertion_order_free() {
        // all owned by frame 3, ties broken by (priority, source, seq, id)
        let mut events =
            vec![ev(4, 3, 5, 0), ev(1, 3, 5, 1), ev(2, 3, -7, 9), ev(3, 3, 5, 0)];
        sort_canonical(&mut events);

        // priority -7 first; inside the priority-5 group the source ids
        // (10, 30, 40 for ev1/ev3/ev4) decide — explicit expectation:
        let got: Vec<u128> = events.iter().map(|e| e.occurrence_id.0.as_u128()).collect();
        assert_eq!(got, vec![2, 1, 3, 4]);

        // shuffle proof (§13.12): ANY permutation sorts to the SAME order
        let mut reversed: Vec<SceneEvent> = events.iter().rev().cloned().collect();
        sort_canonical(&mut reversed);
        assert_eq!(events, reversed);
    }

    #[test]
    fn frame_key_is_exact_frame_then_subframe() {
        // frame-major ✓; within a frame, larger subframe sorts later
        let a = FramePos::new(3, Rational::new(1, 4).unwrap()).unwrap();
        let b = FramePos::new(3, Rational::new(3, 4).unwrap()).unwrap();
        assert!(a < b);
        let c = FramePos::new(4, Rational::zero()).unwrap();
        assert!(b < c, "frame ordering is major over subframe");
    }
}
