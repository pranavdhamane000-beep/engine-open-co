//! Complete simulation state, canonical initialization and frame
//! extraction (spec §14.8).
//!
//! Mutation is confined to initialization, `step_once` and
//! `restore_checkpoint` — nothing else may touch a `SimState`. Particle /
//! spring / body vectors REMAIN SORTED by stable IDs at all times; new
//! entries are inserted in order (insertion stability) and removal is a
//! stable filter. Rendering, hashing and checkpointing never mutate state
//! nor consume random values (§14.6).

use crate::clock::SimTick;
use crate::error::{SimError, SimResult};
use anim_assets::ContentHash;
use anim_core::{domain_hash, NodeId, Rational, SimulationSettings as Settings};
use anim_project::ProjectDocument;
use anim_scene::{SceneCache, SceneId, SceneSnapshot};
use std::collections::BTreeMap;

/// Version tag of the simulation algorithm (§14.7; checkpoint encoding
/// includes it verbatim — bumping it invalidates old checkpoints BY DESIGN).
pub const SIM_ALGORITHM_VERSION: u32 = 1;
pub const PARTICLE_HASH_DOMAIN: &str = "anim_sim/particle_id_v1";

/// One particle. `id` is DERIVED ONCE at spawn from
/// (emitter, generation, spawn_ordinal) — stable and injective.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Particle {
    pub id: u128,
    pub emitter: NodeId,
    pub generation: u32,
    pub spawn_ordinal: u32,
    pub position: [f32; 3],
    pub previous_position: [f32; 3],
    pub velocity: [f32; 3],
    pub age_ticks: u64,
    pub lifetime_ticks: u64,
    pub size: f32,
    pub rotation: f32,
}

/// One spring edge instance (authored edge materialized for the solver).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    pub id: u128,
    pub a: NodeId,
    pub b: NodeId,
    pub rest_length: f32,
    pub stiffness: f32,
}

/// One spring-solver body backed by a scene node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpringBody {
    pub id: NodeId,
    pub position: [f32; 3],
    pub previous_position: [f32; 3],
    pub velocity: [f32; 3],
    pub inverse_mass: f32,
    pub force: [f32; 3],
}

/// The complete simulation state (spec §14.8, WP05 MVP field set).
#[derive(Debug, Clone, PartialEq)]
pub struct SimState {
    pub tick: SimTick,
    pub clock: crate::clock::SimulationClock,
    /// Sorted by `id` ascending at every observable moment.
    pub particles: Vec<Particle>,
    /// Sorted by `id` ascending at every observable moment.
    pub springs: Vec<Spring>,
    /// Sorted by `id` (NodeId) ascending at every observable moment.
    pub bodies: Vec<SpringBody>,
    /// Generation counters reserved for EVERY emitter at initialization.
    pub next_generation: BTreeMap<NodeId, u32>,
    /// Total particles ever spawned per emitter (spawn ordinals are drawn
    /// from it; cursor-free draw addressing needs TOTAL ordinals).
    pub next_ordinal: BTreeMap<NodeId, u32>,
    /// Exact (BigInt) emission accumulator per emitter — `rate·dt` in
    /// steps; carries NO floating point drift across ANY path.
    pub emission: BTreeMap<NodeId, Rational>,
    /// Index of the NEXT unconsumed entry in the flat event schedule.
    pub event_cursor: u64,
    /// Fixed solver iterations (verified ≥ 1 at context build).
    pub solver_iterations: u16,
    /// World gravity in units/second² (copied from settings at init).
    pub gravity: [f32; 3],
}

impl SimState {
    /// Canonical per-tick state hash (§14.7 "Checkpoint hashing"; used
    /// ad hoc by the determinism suite at ANY tick — encoding is identical
    /// to the checkpoint payload minus the envelope).
    pub fn canonical_hash(&self) -> SimResult<ContentHash> {
        crate::checkpoint::encode_state_hash(self)
    }
}

/// Immutable extraction for the renderer (§14.9, §14.10): current AND
/// previous transforms/particles; consuming it NEVER advances state.
#[derive(Debug, Clone, PartialEq)]
pub struct SimulationFrame {
    pub tick: SimTick,
    pub particles: Vec<ParticleDraw>,
    pub bodies: Vec<BodyTransform>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParticleDraw {
    pub id: u128,
    /// The emitter node that produced this particle (renderer entity key;
    /// stable across frames — simulation data is unchanged by extraction).
    pub emitter: NodeId,
    pub position: [f32; 3],
    /// Position at the PREVIOUS tick (exact two-frame pair for velocity).
    pub previous_position: [f32; 3],
    pub size: f32,
    pub rotation: f32,
    /// Absolute tick age at extraction (renderer use: a freshly spawned
    /// particle has age_ticks <= 1 → its fabricated previous position is
    /// NOT a real previous-frame position → velocity validity zero).
    pub age_ticks: u64,
    /// age / lifetime in [0,1); 1 at the final tick (consumers fade out).
    pub age_frac: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyTransform {
    pub id: NodeId,
    pub current: [f32; 3],
    pub previous: [f32; 3],
}

/// Immutable simulation inputs owned by the session (§14.9): authoritative
/// project (READ-ONLY), the scene, compiled schedule, settings, and the
/// checkpoint cache. Checkpoint POLICY lives here, not in simulation
/// results (§14.10).
pub struct SimulationContext {
    pub document: ProjectDocument,
    pub scene: SceneId,
    pub settings: Settings,
    pub schedule: crate::events::TickSchedule,
    /// Internal per-frame scene evaluator cache (performance ONLY — never
    /// affects state values; documented in ADR-0009).
    pub frame_cache: SceneCache,
    pub checkpoint_cache: crate::checkpoint::CheckpointCache,
    initial: Option<SimState>,
}

impl SimulationContext {
    /// Build a session context: validates settings, pins the scene,
    /// compiles the schedule. `cadence_ticks == 0` disables checkpoints.
    pub fn build(
        document: ProjectDocument,
        scene: SceneId,
        cadence_ticks: u64,
        cache_capacity: usize,
    ) -> SimResult<Self> {
        let settings = document.simulation;
        settings.validate().map_err(|e| SimError::InvalidRate(format!("settings: {e}")))?;
        let clock = crate::clock::SimulationClock::new(
            document.frame_rate,
            settings.ticks_per_frame,
        )?;
        let schedule = crate::events::TickSchedule::compile(&document, scene, clock)?;
        Ok(Self {
            document,
            scene,
            settings,
            schedule,
            frame_cache: SceneCache::new(4),
            checkpoint_cache: crate::checkpoint::CheckpointCache::new(
                cadence_ticks,
                cache_capacity,
            ),
            initial: None,
        })
    }

    /// The canonical initial state (tick zero); built ONCE then cloned per
    /// seeker (initial_state is a PURE function of the context).
    pub fn initial(&mut self) -> SimResult<SimState> {
        if let Some(s) = &self.initial {
            return Ok(s.clone());
        }
        let mut cache = std::mem::replace(&mut self.frame_cache, SceneCache::new(0));
        let fp = anim_core::FramePos::new(0, Rational::zero())
            .map_err(|e| SimError::TickOverflow(e.to_string()))?;
        let snapshot = match cache.get_or_evaluate(&self.document, self.scene, fp) {
            Ok(s) => s,
            Err(e) => return Err(SimError::MissingInput(format!("snapshot: {e}"))),
        };
        self.frame_cache = cache;
        let state = initial_state(&snapshot, &self.schedule, self)?;
        self.initial = Some(state.clone());
        Ok(state)
    }
}

/// Canonical initial state construction (spec §14.9). Tick zero carries:
/// materialized springs (sorted by edge id), solver bodies from the
/// evaluated snapshot world translations (sorted by NodeId; inverse mass
/// 1.0 in the WP05 MVP — see ADR-0009), generation counters reserved for
/// EVERY emitter, empty particles, cursor 0.
pub fn initial_state(
    snapshot: &SceneSnapshot,
    _schedule: &crate::events::TickSchedule,
    context: &SimulationContext,
) -> SimResult<SimState> {
    let clock = crate::clock::SimulationClock::new(
        context.document.frame_rate,
        context.settings.ticks_per_frame,
    )?;
    let scene = context
        .document
        .scenes
        .get(&context.scene)
        .ok_or_else(|| SimError::MissingInput(format!("scene {}", context.scene)))?;

    // bodies from spring ENDPOINT nodes (deduped, sorted by NodeId)
    let mut body_ids: BTreeMap<NodeId, ()> = BTreeMap::new();
    for edge in scene.springs.values() {
        body_ids.insert(edge.a, ());
        body_ids.insert(edge.b, ());
    }
    let mut bodies: Vec<SpringBody> = Vec::with_capacity(body_ids.len());
    for id in body_ids.keys() {
        let world = snapshot.nodes.iter().find(|n| &n.id == id).ok_or_else(|| {
            SimError::UnknownEventTarget(format!("body {id} missing from snapshot"))
        })?;
        let position =
            [world.world_transform[0][2] as f32, world.world_transform[1][2] as f32, 0.0];
        if !position.iter().all(|v| v.is_finite()) {
            return Err(SimError::NonfiniteState {
                tick: SimTick::ZERO.0,
                what: format!("body {id} sampled nonfinite origin"),
            });
        }
        bodies.push(SpringBody {
            id: *id,
            position,
            previous_position: position,
            velocity: [0.0; 3],
            inverse_mass: 1.0, // WP05 MVP: uniform mass (ADR-0009)
            force: [0.0; 3],
        });
    }
    // bodies sorted (BTreeMap iteration already key-ascending; assert cheap)
    bodies.sort_by_key(|b| b.id);

    let springs: Vec<Spring> = scene
        .springs
        .iter()
        .map(|(eid, edge)| Spring {
            id: eid.0.as_u128(),
            a: edge.a,
            b: edge.b,
            rest_length: edge.rest_length as f32,
            stiffness: edge.stiffness as f32,
        })
        .collect();

    let mut next_generation = BTreeMap::new();
    let mut next_ordinal = BTreeMap::new();
    let mut emission = BTreeMap::new();
    for node in scene.emitters.keys() {
        next_generation.insert(*node, 0);
        next_ordinal.insert(*node, 0);
        emission.insert(*node, Rational::zero());
    }

    let gravity = [
        context.settings.gravity[0] as f32,
        context.settings.gravity[1] as f32,
        context.settings.gravity[2] as f32,
    ];

    let state = SimState {
        tick: SimTick::ZERO,
        clock,
        particles: Vec::new(),
        springs,
        bodies,
        next_generation,
        next_ordinal,
        emission,
        event_cursor: 0,
        solver_iterations: context.settings.solver_iterations,
        gravity,
    };
    Ok(state)
}

/// Immutable per-frame extraction (§14.9): current/previous particles and
/// bodies, in sorted order. Never mutates state (checked by design — the
/// call takes a shared reference).
pub fn extract_frame(state: &SimState) -> SimResult<SimulationFrame> {
    let mut particles = Vec::with_capacity(state.particles.len());
    for p in &state.particles {
        let vals = [p.position[0], p.position[1], p.position[2], p.size];
        if !vals.iter().all(|v| v.is_finite()) {
            return Err(SimError::NonfiniteState {
                tick: state.tick.0,
                what: format!("particle {}", p.id),
            });
        }
        let lifetime = p.lifetime_ticks.max(1);
        let age_frac = p.age_ticks as f32 / lifetime as f32;
        particles.push(ParticleDraw {
            id: p.id,
            emitter: p.emitter,
            position: p.position,
            previous_position: p.previous_position,
            size: p.size,
            rotation: p.rotation,
            age_ticks: p.age_ticks,
            age_frac: age_frac.clamp(0.0, 1.0),
        });
    }
    let mut bodies = Vec::with_capacity(state.bodies.len());
    for b in &state.bodies {
        bodies.push(BodyTransform {
            id: b.id,
            current: b.position,
            previous: b.previous_position,
        });
    }
    Ok(SimulationFrame { tick: state.tick, particles, bodies })
}

/// Deterministic particle id: BLAKE3 domain hash of (emitter, generation,
/// spawn_ordinal) → u128. Derived ONCE at spawn; never re-derived.
pub fn derive_particle_id(emitter: NodeId, generation: u32, ordinal: u32) -> u128 {
    let mut buf: Vec<u8> = Vec::with_capacity(24);
    buf.extend_from_slice(emitter.0.as_bytes());
    buf.extend_from_slice(&generation.to_le_bytes());
    buf.extend_from_slice(&ordinal.to_le_bytes());
    let digest = domain_hash(PARTICLE_HASH_DOMAIN, &buf);
    let mut digest16 = [0u8; 16];
    digest16.copy_from_slice(&digest[..16]);
    u128::from_le_bytes(digest16)
}
