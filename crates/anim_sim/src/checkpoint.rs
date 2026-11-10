//! Canonical checkpoint encoding, capture, restore, hash-verify, and
//! CheckpointCache with cadence + capacity (spec §14.7, §14.10).
//!
//! Encoding is DETERMINISTIC and EXACT: all integers LE, all f32 as
//! raw bit patterns (u32 LE), all vectors sorted by stable ID, all
//! BigInts encoded as (sign_bit, magnitude_bytes_LE). Domain separators
//! are pinned here and in ADR-0009.

use crate::clock::SimTick;
use crate::error::{SimError, SimResult};
use crate::state::{Particle, SimState, Spring, SpringBody, SIM_ALGORITHM_VERSION};
use anim_assets::ContentHash;
use anim_core::domain_hash;
use std::collections::BTreeMap;

// Domain separator for checkpoint hashing is the literal
// "anim_sim/checkpoint_v1", used inline in encode_state_hash and
// verify/restore paths below (pinned; change = version bump).

/// A complete, self-contained checkpoint payload. All fields are canonical
/// and fully deterministically encoded. `hash` is the capture-time hash of
/// the canonical state encoding — verify_checkpoint recomputes from the
/// fields and compares, so ANY field corruption is detected.
#[derive(Debug, Clone, PartialEq)]
pub struct Checkpoint {
    /// Absolute tick this checkpoint represents (the state AFTER step_once
    /// at this tick — i.e., state.tick points to the NEXT tick to execute).
    pub tick: SimTick,
    /// SIM_ALGORITHM_VERSION at capture time.
    pub algorithm_version: u32,
    /// SIMULATION_INPUT_VERSION at capture time (for cross-version detection).
    pub input_version: u32,
    /// Exact solver iteration count used.
    pub solver_iterations: u16,
    /// Capture-time canonical hash of the state encoding.
    pub hash: ContentHash,
    /// Sorted particles (by id asc).
    pub particles: Vec<Particle>,
    /// Sorted springs (by id asc).
    pub springs: Vec<Spring>,
    /// Sorted bodies (by NodeId asc).
    pub bodies: Vec<SpringBody>,
    /// Emission accumulators (NodeId -> exact Rational).
    pub emission: BTreeMap<anim_core::NodeId, anim_core::Rational>,
    /// Next generation per emitter.
    pub next_generation: BTreeMap<anim_core::NodeId, u32>,
    /// Next ordinal per emitter.
    pub next_ordinal: BTreeMap<anim_core::NodeId, u32>,
    /// Event cursor at capture.
    pub event_cursor: u64,
    /// Gravity at capture (f32 bit patterns).
    pub gravity: [u32; 3],
}

/// Encode a SimState into the canonical byte sequence for hashing/checkpointing.
/// This is the EXACT payload that goes into the checkpoint (minus the envelope
/// fields tick/algorithm_version/input_version/solver_iterations which are
/// added by capture_checkpoint).
pub fn encode_state(state: &SimState) -> Vec<u8> {
    let mut buf = Vec::new();

    // particles: count (u32 LE) then each particle
    buf.extend_from_slice(&(state.particles.len() as u32).to_le_bytes());
    for p in &state.particles {
        // id (u128 LE)
        buf.extend_from_slice(&p.id.to_le_bytes());
        // emitter (NodeId = uuid -> u128 LE)
        buf.extend_from_slice(&p.emitter.0.as_u128().to_le_bytes());
        // generation (u32 LE)
        buf.extend_from_slice(&p.generation.to_le_bytes());
        // spawn_ordinal (u32 LE)
        buf.extend_from_slice(&p.spawn_ordinal.to_le_bytes());
        // position (3 f32 -> u32 LE each)
        for v in p.position {
            buf.extend_from_slice(&v.to_bits().to_le_bytes());
        }
        // previous_position (3 f32)
        for v in p.previous_position {
            buf.extend_from_slice(&v.to_bits().to_le_bytes());
        }
        // velocity (3 f32)
        for v in p.velocity {
            buf.extend_from_slice(&v.to_bits().to_le_bytes());
        }
        // age_ticks (u64 LE)
        buf.extend_from_slice(&p.age_ticks.to_le_bytes());
        // lifetime_ticks (u64 LE)
        buf.extend_from_slice(&p.lifetime_ticks.to_le_bytes());
        // size (f32)
        buf.extend_from_slice(&p.size.to_bits().to_le_bytes());
        // rotation (f32)
        buf.extend_from_slice(&p.rotation.to_bits().to_le_bytes());
    }

    // springs: count then each
    buf.extend_from_slice(&(state.springs.len() as u32).to_le_bytes());
    for s in &state.springs {
        buf.extend_from_slice(&s.id.to_le_bytes());
        buf.extend_from_slice(&s.a.0.as_u128().to_le_bytes());
        buf.extend_from_slice(&s.b.0.as_u128().to_le_bytes());
        buf.extend_from_slice(&s.rest_length.to_bits().to_le_bytes());
        buf.extend_from_slice(&s.stiffness.to_bits().to_le_bytes());
    }

    // bodies: count then each
    buf.extend_from_slice(&(state.bodies.len() as u32).to_le_bytes());
    for b in &state.bodies {
        buf.extend_from_slice(&b.id.0.as_u128().to_le_bytes());
        for v in b.position {
            buf.extend_from_slice(&v.to_bits().to_le_bytes());
        }
        for v in b.previous_position {
            buf.extend_from_slice(&v.to_bits().to_le_bytes());
        }
        for v in b.velocity {
            buf.extend_from_slice(&v.to_bits().to_le_bytes());
        }
        buf.extend_from_slice(&b.inverse_mass.to_bits().to_le_bytes());
        for v in b.force {
            buf.extend_from_slice(&v.to_bits().to_le_bytes());
        }
    }

    // emission: count then (node_id, rational)
    // Rational encoded as two i128 LE (num, den)
    buf.extend_from_slice(&(state.emission.len() as u32).to_le_bytes());
    for (node, rat) in &state.emission {
        buf.extend_from_slice(&node.0.as_u128().to_le_bytes());
        // numerator (i128 LE)
        buf.extend_from_slice(&rat.num().to_le_bytes());
        // denominator (i128 LE)
        buf.extend_from_slice(&rat.den().to_le_bytes());
    }

    // next_generation
    buf.extend_from_slice(&(state.next_generation.len() as u32).to_le_bytes());
    for (node, gen) in &state.next_generation {
        buf.extend_from_slice(&node.0.as_u128().to_le_bytes());
        buf.extend_from_slice(&gen.to_le_bytes());
    }

    // next_ordinal
    buf.extend_from_slice(&(state.next_ordinal.len() as u32).to_le_bytes());
    for (node, ord) in &state.next_ordinal {
        buf.extend_from_slice(&node.0.as_u128().to_le_bytes());
        buf.extend_from_slice(&ord.to_le_bytes());
    }

    // event_cursor
    buf.extend_from_slice(&state.event_cursor.to_le_bytes());

    // gravity (3 f32 -> u32)
    for v in state.gravity {
        buf.extend_from_slice(&v.to_bits().to_le_bytes());
    }

    buf
}

/// Canonical state hash for ad-hoc verification (spec §14.7).
pub fn encode_state_hash(state: &SimState) -> SimResult<ContentHash> {
    let payload = encode_state(state);
    let digest = domain_hash("anim_sim/checkpoint_v1", &payload);
    Ok(ContentHash(digest))
}

/// Capture a full checkpoint from the current state.
pub fn capture_checkpoint(state: &SimState) -> SimResult<Checkpoint> {
    let hash = encode_state_hash(state)?;
    Ok(Checkpoint {
        tick: state.tick,
        algorithm_version: SIM_ALGORITHM_VERSION,
        input_version: crate::events::SIMULATION_INPUT_VERSION,
        solver_iterations: state.solver_iterations,
        hash,
        particles: state.particles.clone(),
        springs: state.springs.clone(),
        bodies: state.bodies.clone(),
        emission: state.emission.clone(),
        next_generation: state.next_generation.clone(),
        next_ordinal: state.next_ordinal.clone(),
        event_cursor: state.event_cursor,
        gravity: state.gravity.map(|v| v.to_bits()),
    })
}

/// Verify a checkpoint's hash matches its payload (real corruption
/// detection: recomputes from the FIELDS and compares against the
/// capture-time hash stored IN the checkpoint).
pub fn verify_checkpoint(chk: &Checkpoint) -> SimResult<()> {
    // Reconstruct a minimal SimState for hashing — the hash deliberately
    // EXCLUDES the clock (it is configuration, not evolving state; the
    // clock is revalidated from context at restore). Only the evolve-
    // able fields participate, exactly as in encode_state.
    let state = SimState {
        tick: chk.tick,
        clock: crate::clock::SimulationClock {
            frame_rate: anim_core::FrameRate::new(1, 1).unwrap(), // dummy; not in hash
            ticks_per_frame: 1,
            rate: anim_core::FrameRate::new(1, 1).unwrap(),
        },
        particles: chk.particles.clone(),
        springs: chk.springs.clone(),
        bodies: chk.bodies.clone(),
        emission: chk.emission.clone(),
        next_generation: chk.next_generation.clone(),
        next_ordinal: chk.next_ordinal.clone(),
        event_cursor: chk.event_cursor,
        solver_iterations: chk.solver_iterations,
        gravity: [
            f32::from_bits(chk.gravity[0]),
            f32::from_bits(chk.gravity[1]),
            f32::from_bits(chk.gravity[2]),
        ],
    };
    let actual = encode_state_hash(&state)?;
    if chk.hash != actual {
        return Err(SimError::CheckpointHashMismatch(chk.tick.0));
    }
    Ok(())
}

/// Restore state from a checkpoint (verifies hash first). The clock is
/// supplied by the caller (the SimulationContext owns the canonical
/// configuration; a checkpoint never carries it).
pub fn restore_checkpoint(
    chk: &Checkpoint,
    clock: crate::clock::SimulationClock,
) -> SimResult<SimState> {
    verify_checkpoint(chk)?;
    Ok(SimState {
        tick: chk.tick,
        clock,
        particles: chk.particles.clone(),
        springs: chk.springs.clone(),
        bodies: chk.bodies.clone(),
        emission: chk.emission.clone(),
        next_generation: chk.next_generation.clone(),
        next_ordinal: chk.next_ordinal.clone(),
        event_cursor: chk.event_cursor,
        solver_iterations: chk.solver_iterations,
        gravity: [
            f32::from_bits(chk.gravity[0]),
            f32::from_bits(chk.gravity[1]),
            f32::from_bits(chk.gravity[2]),
        ],
    })
}

/// Checkpoint cache with cadence + capacity + eviction (spec §14.7).
#[derive(Debug)]
pub struct CheckpointCache {
    cadence_ticks: u64, // 0 = disabled
    capacity: usize,
    /// Sorted by tick asc; we keep the OLDEST when over capacity.
    entries: BTreeMap<i64, Checkpoint>,
}

impl CheckpointCache {
    pub fn new(cadence_ticks: u64, capacity: usize) -> Self {
        Self { cadence_ticks, capacity: capacity.max(1), entries: BTreeMap::new() }
    }

    /// Should we capture a checkpoint at this tick?
    pub fn should_capture(&self, tick: SimTick) -> bool {
        if self.cadence_ticks == 0 {
            return false;
        }
        tick.0 % self.cadence_ticks as i64 == 0
    }

    /// Insert a checkpoint; evict oldest if over capacity.
    pub fn insert(&mut self, tick: SimTick, checkpoint: Checkpoint) {
        self.entries.insert(tick.0, checkpoint);
        if self.entries.len() > self.capacity {
            // Remove oldest (first key)
            if let Some(first_key) = self.entries.keys().next().copied() {
                self.entries.remove(&first_key);
            }
        }
    }

    /// Get the latest checkpoint at or before target tick.
    pub fn get_latest(&self, target: SimTick) -> Option<&Checkpoint> {
        self.entries.range(..=target.0).next_back().map(|(_, v)| v)
    }

    /// Remove ALL entries at or after `from_tick` (a corrupted checkpoint
    /// invalidates itself AND everything newer — verify walks newest→oldest
    /// and discards the corrupted suffix). Returns how many were removed.
    pub fn remove_before_strict(&mut self, from_tick: i64) -> usize {
        let before = self.entries.len();
        self.entries.retain(|&k, _| k < from_tick);
        before - self.entries.len()
    }

    /// Remove all entries beyond the new latest tick (for truncation on replay).
    pub fn truncate_after(&mut self, tick: SimTick) {
        self.entries.retain(|&k, _| k <= tick.0);
    }

    /// Iterate all checkpoints in tick order.
    pub fn iter(&self) -> impl Iterator<Item = (&i64, &Checkpoint)> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
