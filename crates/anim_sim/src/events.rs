//! Flat event schedule, canonical ordering, SIMULATION_INPUT_VERSION
//! (spec §14.5, §14.7 "Simulation input version").
//!
//! The schedule is a FLAT Vec of (tick, event_kind, event_payload) sorted
//! by the canonical order: (tick ASC, priority ASC, source ASC, local_seq
//! ASC, EventOccurrenceId ASC). This is a COMPILED artifact — the ONLY
//! authoritative source at runtime; evaluator output is never consulted
//! again. Building it is a pure function of the project document + scene +
//! clock settings (§14.5).

use crate::clock::{SimTick, SimulationClock};
use crate::error::{SimError, SimResult};
use anim_core::{EventDefinitionId, EventOccurrenceId, NodeId, SceneId};
use anim_project::{EventKind, ProjectDocument};
use std::collections::BTreeMap;

/// Version of the compiled simulation input schema. Bump ONLY when the
/// canonical sort key, payload encoding or emitted event set changes —
/// never for internal optimizations.
pub const SIMULATION_INPUT_VERSION: u32 = 1;

/// One entry in the flat compiled schedule. `tick` is the ABSOLUTE tick
/// this event fires (ceil(t·rate) for the event's time). `priority` is
/// the event's priority (lower = earlier). `source` is the defining node's
/// NodeId (for stable tiebreak). `local_seq` is the index of this event
/// within its definition (definition emits multiple occurrences over time).
/// The id is the occurrence's BLAKE3-derived EventOccurrenceId (§14.5
/// canonical occurrence identity).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduledEvent {
    pub tick: SimTick,
    pub kind: EventKind,
    pub priority: i32,
    pub source: NodeId,
    pub local_seq: u32,
    pub occurrence_id: EventOccurrenceId,
    /// Kind-specific payload (versioned with SIMULATION_INPUT_VERSION).
    pub payload: EventPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventPayload {
    Spawn { emitter: NodeId },
    Cue { cue_id: u32 },
    Note { text_hash: [u8; 16] },
}

/// The complete flat schedule for one scene + clock setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TickSchedule {
    /// All events sorted by canonical order.
    pub events: Vec<ScheduledEvent>,
    /// Tick at which the first event occurs (for fast-forward bounds).
    pub first_tick: Option<SimTick>,
    /// Tick past the last event (for early-exit bounds).
    pub last_tick_plus_one: Option<SimTick>,
    /// Map from EventDefinitionId → (first local_seq, occurrence count)
    /// for fast cursor management. Not in canonical ordering — bookkeeping.
    pub definition_index: BTreeMap<EventDefinitionId, (u32, u32)>,
}

impl TickSchedule {
    /// Compile the flat schedule from a project document + scene + clock.
    /// Pure function — no side effects, no random draws, no wall clock.
    pub fn compile(
        document: &ProjectDocument,
        scene_id: SceneId,
        clock: SimulationClock,
    ) -> SimResult<Self> {
        let scene = document
            .scenes
            .get(&scene_id)
            .ok_or_else(|| SimError::UnknownEventTarget(format!("scene {scene_id}")))?;

        let mut all: Vec<ScheduledEvent> = Vec::new();
        let mut def_index = BTreeMap::new();

        // Iterate event definitions in NodeId order (stable source order)
        for (def_id, def) in &scene.events {
            let priority = def.priority;
            let kind = def.kind;
            // Each definition generates occurrences at its scheduled times.
            // Occurrence i fires at time t_i = def.times[i].
            // tick = ceil(t_i * rate).
            let rate_num = clock.rate.num as i128;
            let rate_den = clock.rate.den as i128;

            for (local_seq, time) in def.times.iter().enumerate() {
                let tick_val =
                    (time.num() * rate_den + rate_num - 1) / (time.den() * rate_num);
                let tick = SimTick(tick_val.try_into().map_err(|_| {
                    SimError::TickOverflow(format!("tick from {time} × rate"))
                })?);
                let occurrence_id = derive_event_occurrence_id(def_id, local_seq as u32);

                let payload = match kind {
                    EventKind::Spawn => EventPayload::Spawn {
                        emitter: def.source, // emitter IS the source node
                    },
                    EventKind::Cue => EventPayload::Cue {
                        cue_id: def.asset.map(|a| a.0.as_u128() as u32).unwrap_or(0),
                    },
                    EventKind::Note => {
                        // stable 16-byte hash of the definition name
                        // (short names zero-padded; never panics)
                        let bytes = def.name.as_bytes();
                        let mut text_hash = [0u8; 16];
                        let n = bytes.len().min(16);
                        text_hash[..n].copy_from_slice(&bytes[..n]);
                        EventPayload::Note { text_hash }
                    }
                };

                all.push(ScheduledEvent {
                    tick,
                    kind,
                    priority,
                    source: def.source,
                    local_seq: local_seq as u32,
                    occurrence_id,
                    payload,
                });
            }
            def_index.insert(*def_id, (0, def.times.len() as u32));
        }

        // Canonical sort: (tick ASC, priority ASC, source ASC, local_seq ASC, id ASC)
        all.sort_by(|a, b| {
            a.tick
                .0
                .cmp(&b.tick.0)
                .then(a.priority.cmp(&b.priority))
                .then(a.source.0.as_u128().cmp(&b.source.0.as_u128()))
                .then(a.local_seq.cmp(&b.local_seq))
                .then(a.occurrence_id.0.as_u128().cmp(&b.occurrence_id.0.as_u128()))
        });

        let first_tick = all.first().map(|e| e.tick);
        let last_tick_plus_one = all.last().map(|e| SimTick(e.tick.0 + 1));

        Ok(Self {
            events: all,
            first_tick,
            last_tick_plus_one,
            definition_index: def_index,
        })
    }

    /// Find the index of the first event with tick >= target (lower bound).
    /// Used by the step loop's event cursor.
    pub fn lower_bound_tick(&self, target: SimTick) -> usize {
        self.events.partition_point(|e| e.tick.0 < target.0)
    }
}

/// Derive EventOccurrenceId from definition id + local sequence.
/// Domain: "anim_engine/event_occurrence_v1" ∥ EventDefinitionId ∥ u32 LE.
pub fn derive_event_occurrence_id(
    def_id: &EventDefinitionId,
    local_seq: u32,
) -> EventOccurrenceId {
    let mut buf: Vec<u8> = Vec::with_capacity(20);
    buf.extend_from_slice(def_id.0.as_bytes());
    buf.extend_from_slice(&local_seq.to_le_bytes());
    let digest = anim_core::domain_hash("anim_engine/event_occurrence_v1", &buf);
    EventOccurrenceId(uuid::Uuid::from_bytes(digest[..16].try_into().expect("16 bytes")))
}
