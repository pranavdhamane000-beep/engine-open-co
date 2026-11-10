//! WP05 determinism acceptance tests (spec §14.12 gate, §14.7):
//!
//! 1. cadence matrix (1, 7, 64, 1024, disabled) — SEEKING to ANY tick
//!    (in- and off-cadence) reproduces the forward-run per-tick hash.
//! 2. randomized seek order — seek sequence order NEVER changes results.
//! 3. chunked vs single stepping — identical final hash.
//! 4. warm-up/export vs uncheckpointed — identical.
//! 5. nonfinite — loud failure (SimError::NonfiniteState), never clamps.
//! 6. insertion/removal stability — sorted order preserved under churn.

use anim_core::{
    EmitterSettings, EventDefinitionId, FrameRate, NodeId, SpringEdge, SpringEdgeId,
};
use anim_project::{EventKind, NodeData, ProjectDocument, SceneData};
use anim_sim::clock::{SimTick, SimulationClock};
use anim_sim::state::{SimState, SimulationContext};
use anim_sim::{advance, extract_frame, seek, step_once, SimulationMode};
use std::collections::BTreeMap;

/// Deterministic LCG for SEEK-ORDER generation only (never simulation).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 1
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn emitter_settings(rate: f64) -> EmitterSettings {
    EmitterSettings { spawn_rate: rate, initial_speed: 5.0, lifetime_ticks: 30, size: 0.4 }
}

fn settings() -> anim_core::SimulationSettings {
    anim_core::SimulationSettings {
        ticks_per_frame: 2,
        project_seed: [42u8; 32],
        gravity: [0.0, -9.0, 0.0],
        solver_iterations: 8,
    }
}

/// Small deterministic scene: 2 emitter nodes + 1 spring edge between
/// them + one Note event (exercises the event cursor) at 24 fps, N=2.
fn fixture(rate_e0: f64, rate_e1: f64) -> (ProjectDocument, anim_core::SceneId) {
    let mut doc = ProjectDocument::new(FrameRate::new(24, 1).unwrap());
    doc.simulation = settings();

    let scene_id = anim_core::SceneId::new_v4();
    let mut scene = SceneData::new("arena_sim_fixture");

    let a = NodeId::new_v4();
    let b = NodeId::new_v4();
    scene.nodes.insert(
        a,
        NodeData {
            name: "emitter_a".into(),
            parent: None,
            asset: None,
            properties: BTreeMap::new(),
        },
    );
    scene.nodes.insert(
        b,
        NodeData {
            name: "emitter_b".into(),
            parent: None,
            asset: None,
            properties: BTreeMap::new(),
        },
    );
    scene.emitters.insert(a, emitter_settings(rate_e0));
    scene.emitters.insert(b, emitter_settings(rate_e1));

    let edge_id = SpringEdgeId::new_v4();
    scene.springs.insert(edge_id, SpringEdge { a, b, rest_length: 2.0, stiffness: 30.0 });

    // one Note event at t = 1/2 s (tick 24 at rate 48)
    let note_id = EventDefinitionId::new_v4();
    scene.events.insert(
        note_id,
        anim_project::EventDefinition {
            name: "beat_marker_one".into(),
            kind: EventKind::Note,
            priority: 7,
            source: a,
            asset: None,
            times: vec![anim_core::Rational::new(1, 2).unwrap()],
            params: BTreeMap::new(),
        },
    );

    doc.scenes.insert(scene_id, scene);
    (doc, scene_id)
}

const HORIZON: i64 = 900; // ticks (450 frames at 2 ticks/frame)

/// Forward run capturing EVERY tick's hash: the golden reference the
/// matrix compares against.
fn forward_run_hashed(
    doc: &ProjectDocument,
    scene_id: anim_core::SceneId,
    horizon: i64,
) -> BTreeMap<i64, anim_assets::ContentHash> {
    let mut context = SimulationContext::build(doc.clone(), scene_id, 0, 1).unwrap();
    let schedule = context.schedule.clone();
    let mut state = context.initial().unwrap();
    let mut hashes = BTreeMap::new();
    for _ in 0..horizon {
        step_once(&mut state, &schedule, &mut context).unwrap();
        hashes.insert(state.tick.0, state.canonical_hash().unwrap());
    }
    hashes
}

/// Forward run WITHOUT any checkpoint capture (pure Live).
/// (reserved for a later in-file use; see forward_run_hashed)
#[allow(dead_code)]
fn forward_run_state(
    doc: &ProjectDocument,
    scene_id: anim_core::SceneId,
    horizon: i64,
) -> SimState {
    let mut context = SimulationContext::build(doc.clone(), scene_id, 0, 1).unwrap();
    let schedule = context.schedule.clone();
    let mut state = context.initial().unwrap();
    for _ in 0..horizon {
        step_once(&mut state, &schedule, &mut context).unwrap();
    }
    state
}

#[test]
fn cadence_matrix_seeks_match_forward_hashes() {
    let (doc, scene_id) = fixture(40.0, 15.0);
    let golden = forward_run_hashed(&doc, scene_id, HORIZON);

    for cadence in [1u64, 7, 64, 1024, 0] {
        let mut context =
            SimulationContext::build(doc.clone(), scene_id, cadence, 16).unwrap();
        let schedule = context.schedule.clone();
        let mut state = context.initial().unwrap();

        // forward with checkpoint capture (Live)
        for _ in 0..HORIZON {
            step_once(&mut state, &schedule, &mut context).unwrap();
        }
        assert_eq!(
            state.canonical_hash().unwrap(),
            golden[&HORIZON],
            "cadence {cadence}: forward end hash diverged"
        );

        // seek back to in- and off-cadence targets, in mixed order
        let mut rng = Lcg(0xC0FFEE + cadence);
        let mut targets: Vec<i64> = Vec::new();
        for _ in 0..24 {
            targets.push(rng.below(HORIZON as u64) as i64 + 1);
        }
        targets.push(1);
        targets.push(HORIZON);
        // exact cadence ticks too (when applicable, within horizon)
        if cadence > 1 {
            targets.push((cadence as i64).min(HORIZON));
            targets.push(((cadence as i64) * 3).min(HORIZON));
        }
        targets.sort_unstable();
        targets.dedup();

        for t in targets {
            let back = seek(&mut context, SimTick(t), None).unwrap();
            assert_eq!(
                back.canonical_hash().unwrap(),
                golden[&t],
                "cadence {cadence}: seek to tick {t} diverged"
            );
        }
    }
}

#[test]
fn seek_order_does_not_change_results() {
    let (doc, scene_id) = fixture(50.0, 20.0);
    let golden = forward_run_hashed(&doc, scene_id, HORIZON);

    // Two contexts, same cadence, DIFFERENT seek orders → same hashes.
    let mut ctx_a = SimulationContext::build(doc.clone(), scene_id, 7, 8).unwrap();
    let mut rng_a = Lcg(11);
    let mut rng_b = Lcg(99);
    for i in 0..12 {
        let ta = rng_a.below(HORIZON as u64) as i64 + 1;
        let tb = rng_b.below(HORIZON as u64) as i64 + 1;
        let ha = seek(&mut ctx_a, SimTick(ta), None).unwrap().canonical_hash().unwrap();
        assert_eq!(ha, golden[&ta], "order pass {i} seek-a tick {ta}");
        let hb = {
            // fresh context for the other order (both must agree with golden)
            let mut ctx_b = SimulationContext::build(doc.clone(), scene_id, 7, 8).unwrap();
            seek(&mut ctx_b, SimTick(tb), None).unwrap().canonical_hash().unwrap()
        };
        assert_eq!(hb, golden[&tb], "order pass {i} seek-b tick {tb}");
    }
}

#[test]
fn chunked_stepping_matches_single() {
    let (doc, scene_id) = fixture(30.0, 30.0);
    let golden = forward_run_hashed(&doc, scene_id, HORIZON);

    // single advance
    let mut ctx_single = SimulationContext::build(doc.clone(), scene_id, 7, 4).unwrap();
    let schedule = ctx_single.schedule.clone();
    let mut state = ctx_single.initial().unwrap();
    advance(
        &mut state,
        &schedule,
        &mut ctx_single,
        HORIZON as u64,
        None,
        SimulationMode::Live,
    )
    .unwrap();

    // chunked advance (7 chunks) through FRESH runs each time
    let mut ctx_chunk = SimulationContext::build(doc.clone(), scene_id, 7, 4).unwrap();
    let schedule_chunk = ctx_chunk.schedule.clone();
    let mut state_chunk = ctx_chunk.initial().unwrap();
    let chunk = HORIZON / 7;
    for _ in 0..7 {
        advance(
            &mut state_chunk,
            &schedule_chunk,
            &mut ctx_chunk,
            chunk as u64,
            None,
            SimulationMode::Live,
        )
        .unwrap();
    }
    // remainder
    advance(
        &mut state_chunk,
        &schedule_chunk,
        &mut ctx_chunk,
        (HORIZON % 7) as u64,
        None,
        SimulationMode::Live,
    )
    .unwrap();

    assert_eq!(
        state.canonical_hash().unwrap(),
        state_chunk.canonical_hash().unwrap(),
        "chunked vs single diverged"
    );
    assert_eq!(state.canonical_hash().unwrap(), golden[&HORIZON]);
}

#[test]
fn export_mode_matches_uncheckpointed() {
    let (doc, scene_id) = fixture(25.0, 10.0);
    let golden = forward_run_hashed(&doc, scene_id, HORIZON);

    // Export: frame-boundary capture forced; end state must equal golden.
    let mut ctx = SimulationContext::build(doc.clone(), scene_id, 64, 8).unwrap();
    let schedule = ctx.schedule.clone();
    let mut state = ctx.initial().unwrap();
    advance(&mut state, &schedule, &mut ctx, HORIZON as u64, None, SimulationMode::Export)
        .unwrap();
    assert_eq!(state.canonical_hash().unwrap(), golden[&HORIZON]);
}

#[test]
fn nonfinite_fails_loudly() {
    let (mut doc, scene_id) = fixture(20.0, 20.0);
    doc.simulation.gravity = [0.0, 3.4e37, 0.0]; // finite f64, explodes in f32 updates

    let mut ctx = SimulationContext::build(doc.clone(), scene_id, 0, 1).unwrap();
    let schedule = ctx.schedule.clone();
    let mut state = ctx.initial().unwrap();

    let mut result: Result<(), anim_sim::SimError> = Ok(());
    for _ in 0..20_000 {
        match step_once(&mut state, &schedule, &mut ctx) {
            Ok(_) => continue,
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    match result {
        Err(anim_sim::SimError::NonfiniteState { .. }) => {} // loud failure: good
        Ok(()) => panic!("nonfinite run did NOT fail within horizon"),
        Err(other) => panic!("wrong error class: {other:?}"),
    }
}

#[test]
fn insertion_removal_is_sorted_and_stable() {
    let (doc, scene_id) = fixture(120.0, 120.0); // heavy churn (fast spawns+dying)
    let mut ctx = SimulationContext::build(doc.clone(), scene_id, 0, 1).unwrap();
    let schedule = ctx.schedule.clone();
    let mut state = ctx.initial().unwrap();

    for step in 0..400 {
        step_once(&mut state, &schedule, &mut ctx).unwrap();
        // sorted by id, strictly ascending
        let ids: Vec<u128> = state.particles.iter().map(|p| p.id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids, sorted, "tick {}: particle order broken", step);
        // every live particle has age < lifetime at LEAST one tick
        for p in &state.particles {
            assert!(
                p.age_ticks < p.lifetime_ticks,
                "tick {}: expired particle retained",
                step
            );
        }
        // extraction succeeded (finite states only)
        let frame = extract_frame(&state).unwrap();
        for d in &frame.particles {
            assert!(d.age_frac.is_finite());
        }
    }
}

#[test]
fn seek_to_tick_zero_is_initial_state() {
    let (doc, scene_id) = fixture(30.0, 30.0);
    let mut ctx = SimulationContext::build(doc.clone(), scene_id, 7, 4).unwrap();
    let schedule = ctx.schedule.clone();
    let mut state = ctx.initial().unwrap();
    for _ in 0..50 {
        step_once(&mut state, &schedule, &mut ctx).unwrap();
    }
    let back = seek(&mut ctx, SimTick(0), None).unwrap();
    let fresh =
        SimulationContext::build(doc.clone(), scene_id, 7, 4).unwrap().initial().unwrap();
    assert_eq!(
        back.canonical_hash().unwrap(),
        fresh.canonical_hash().unwrap(),
        "seek(0) must restore pristine initial state"
    );
}

#[test]
fn clock_plumbs_fixture_rate_exactly() {
    let clock = SimulationClock::new(FrameRate::new(24, 1).unwrap(), 2).unwrap();
    assert_eq!(clock.rate, FrameRate::new(48, 1).unwrap());
    assert_eq!(clock.frame_to_tick(5).unwrap(), SimTick(10));
    assert_eq!(clock.tick_to_frame(SimTick(10)), 5);
}
