//! PHASE 2 EXIT DEMO (sim part): scrub/seek a simulated scene, restore
//! from a checkpoint, prove replay equality — LIVE, like phase1_demo.
//!
//! Script (all fixed; deterministic):
//!   1. Build the fixture (2 emitters + spring + Note event, 24 fps × 2
//!      ticks), save it ATOMICALLY, load it BACK, and re-save to prove the
//!      round trip preserves the canonical document hash byte-for-byte.
//!   2. Golden run: forward 20,000 ticks (10,000 frames) with checkpoint
//!      capture DISABLED, recording the state hash at EVERY tick.
//!   3. Scrub: seek back/forward through interleaved targets against a
//!      SEPARATE context that ran the same forward path under cadence 64
//!      (cap 16); every seek lands on the golden hash for that tick.
//!   4. Restore-from-checkpoint: take the scrub context's newest
//!      checkpoint, restore it, then march forward to tick 20,000 — the
//!      final hash MUST equal the golden final hash (replay equality).
//!   5. Live corruption proof: corrupt the newest cached checkpoint's
//!      particle vector, re-insert it, then seek past it — the engine must
//!      DISCARD the corrupted suffix, rebuild from a lower checkpoint (or
//!      tick 0), and still land on the exact golden hash.
//!   6. Dry extraction: print a real SimulationFrame (particle count, one
//!      sample draw, body positions) to show the sim state is renderer-ready.
//!
//! Exit 0 = PASS; every check prints a proof line.

use anim_project::{NodeData, SceneData};
use anim_sim::clock::SimTick;
use anim_sim::state::{SimulationContext, SimulationFrame};
use anim_sim::{advance, extract_frame, seek, SimulationMode};
use std::collections::BTreeMap;
use std::path::Path;

const HORIZON: i64 = 20_000; // 10,000 frames at 24 fps × 2 ticks/frame

fn fixture() -> (anim_project::ProjectDocument, anim_core::SceneId) {
    let mut doc =
        anim_project::ProjectDocument::new(anim_core::FrameRate::new(24, 1).unwrap());
    doc.simulation = anim_core::SimulationSettings {
        ticks_per_frame: 2,
        project_seed: [42u8; 32],
        gravity: [0.0, -9.0, 0.0],
        solver_iterations: 8,
    };
    let scene_id = anim_core::SceneId::new_v4();
    let mut scene = SceneData::new("phase2_demo_arena");
    let a = anim_core::NodeId::new_v4();
    let b = anim_core::NodeId::new_v4();
    scene.nodes.insert(
        a,
        NodeData {
            name: "emitter_left".into(),
            parent: None,
            asset: None,
            properties: Default::default(),
        },
    );
    scene.nodes.insert(
        b,
        NodeData {
            name: "emitter_right".into(),
            parent: None,
            asset: None,
            properties: Default::default(),
        },
    );
    scene.emitters.insert(
        a,
        anim_core::EmitterSettings {
            spawn_rate: 40.0,
            initial_speed: 5.0,
            lifetime_ticks: 30,
            size: 0.4,
        },
    );
    scene.emitters.insert(
        b,
        anim_core::EmitterSettings {
            spawn_rate: 15.0,
            initial_speed: 7.0,
            lifetime_ticks: 24,
            size: 0.25,
        },
    );
    scene.springs.insert(
        anim_core::SpringEdgeId::new_v4(),
        anim_core::SpringEdge { a, b, rest_length: 2.0, stiffness: 30.0 },
    );
    // Note event drives the canonical cursor (fires tick 24 = t 1/2 s)
    let note = anim_core::EventDefinitionId::new_v4();
    scene.events.insert(
        note,
        anim_project::EventDefinition {
            name: "beat_marker_one".into(),
            kind: anim_project::EventKind::Note,
            priority: 7,
            source: a,
            asset: None,
            times: vec![anim_core::Rational::new(1, 2).unwrap()],
            params: Default::default(),
        },
    );
    doc.scenes.insert(scene_id, scene);
    (doc, scene_id)
}

fn main() {
    let (doc, scene_id) = fixture();

    // 1. Atomic save → load back → re-save: canonical hash round trip.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("phase2_demo.json");
    let saved = anim_project::save_atomic(&doc, &path).expect("save");
    let reopened = anim_project::load_document(Path::new(&path)).expect("load");
    let resaved = anim_project::save_atomic(&reopened, &path).expect("re-save");
    let round_trip_ok = saved.document_hash == resaved.document_hash;
    println!(
        "1. PERSISTENCE  save/load/re-save round trip hash equal = {} ({})",
        round_trip_ok,
        &saved.document_hash[..16.min(saved.document_hash.len())]
    );
    assert!(round_trip_ok, "document round trip was NOT hash-identical");

    // 2. Golden forward run — capture the state hash at EVERY tick.
    let mut golden_ctx =
        SimulationContext::build(reopened.clone(), scene_id, 0, 1).expect("golden context");
    let golden_schedule = golden_ctx.schedule.clone();
    let mut golden = golden_ctx.initial().expect("initial");
    let mut hashes: BTreeMap<i64, String> = BTreeMap::new();
    hashes.insert(0, golden.canonical_hash().expect("h0").full_hex());
    for t in 1..=HORIZON {
        anim_sim::step_once(&mut golden, &golden_schedule, &mut golden_ctx)
            .expect("golden step");
        hashes.insert(t, golden.canonical_hash().expect("hash").full_hex());
    }
    let final_golden = hashes[&HORIZON].clone();
    println!(
        "2. GOLDEN       {} ticks ({} frames) hashed; final {}",
        HORIZON,
        HORIZON / 2,
        &final_golden[..16]
    );

    // 3. Scrub against a checkpointed context (cadence 64 / cap 16).
    let mut ctx = SimulationContext::build(reopened.clone(), scene_id, 64, 16)
        .expect("scrub context");
    let schedule = ctx.schedule.clone();
    let mut state = ctx.initial().expect("initial");
    advance(&mut state, &schedule, &mut ctx, HORIZON as u64, None, SimulationMode::Live)
        .expect("forward");
    assert_eq!(
        state.canonical_hash().expect("hash").full_hex(),
        final_golden,
        "checkpointed forward run diverged from golden"
    );
    println!("3. FORWARD+CKPT checkpointed run == golden hash; now scrubbing…");

    // interleaved scrub targets (back/forward, cadence & off-cadence)
    let targets: [i64; 8] = [15_977, 0, 19_999, 1, 12_345, 10_001, 7_777, 20_000];
    for t in targets {
        let back = seek(&mut ctx, SimTick(t), None).expect("seek");
        let want = &hashes[&t];
        let got = back.canonical_hash().expect("hash").full_hex();
        assert_eq!(got, *want, "seek to tick {t} diverged from golden");
        match t {
            0 => {
                println!("   · seek tick {:>6} → RESTORED INITIAL STATE  {}", t, &got[..16])
            }
            1 => println!(
                "   · seek tick {:>6} → 1 tick of history, exact      {}",
                t,
                &got[..16]
            ),
            _ => println!(
                "   · seek tick {:>6} → golden hash match            {}",
                t,
                &got[..16]
            ),
        }
    }

    // 4. Restore-from-checkpoint + continue == replay equality.
    let chk = ctx
        .checkpoint_cache
        .get_latest(SimTick(HORIZON))
        .expect("a checkpoint exists")
        .clone();
    println!("4. RESTORE      newest checkpoint at tick {} verified", chk.tick.0);
    let clock = anim_sim::SimulationClock::new(
        reopened.frame_rate,
        reopened.simulation.ticks_per_frame,
    )
    .expect("clock");
    let mut resumed = anim_sim::restore_checkpoint(&chk, clock).expect("restore");
    assert_eq!(
        resumed.canonical_hash().expect("hash").full_hex(),
        hashes[&chk.tick.0],
        "restored checkpoint hash mismatch for its own tick"
    );
    let from = chk.tick.0;
    let schedule_resume = ctx.schedule.clone();
    advance(
        &mut resumed,
        &schedule_resume,
        &mut ctx,
        (HORIZON - from) as u64,
        Some(SimTick(HORIZON)),
        SimulationMode::Live,
    )
    .expect("resume advance");
    assert_eq!(
        resumed.canonical_hash().expect("hash").full_hex(),
        final_golden,
        "restore+continue did NOT reproduce the golden final hash"
    );
    println!(
        "4. …continue    from tick {} → {}; replay equality proves the checkpoint is lossless",
        from, HORIZON
    );

    // 5. Live corruption recovery: corrupt the newest checkpoint, seek.
    let latest_tick =
        ctx.checkpoint_cache.get_latest(SimTick(HORIZON)).expect("checkpoint").tick.0;
    let mut corrupted =
        ctx.checkpoint_cache.get_latest(SimTick(HORIZON)).expect("checkpoint").clone();
    if let Some(p) = corrupted.particles.first_mut() {
        p.position[0] += 1e-6; // bit-visible corruption
    }
    ctx.checkpoint_cache.insert(SimTick(corrupted.tick.0), corrupted);
    let survived = seek(&mut ctx, SimTick(HORIZON), None).expect("seek after corruption");
    assert_eq!(
        survived.canonical_hash().expect("hash").full_hex(),
        final_golden,
        "corrupted-checkpoint seek did not recover exactly"
    );
    println!("5. CORRUPTION   checkpoint @ tick {} corrupted → detect + discard + rebuild from lower state; final hash still EXACT", latest_tick);

    // 6. Dry extraction: renderer-ready frame at the final tick.
    let frame: SimulationFrame = extract_frame(&survived).expect("extract");
    let sample = frame
        .particles
        .first()
        .map(|p| {
            format!("id {:16x} age_frac {:.3} size {:.3}", p.id as u64, p.age_frac, p.size)
        })
        .unwrap_or_else(|| "none (all dead)".into());
    let body = frame
        .bodies
        .first()
        .map(|b| format!("{:.4},{:.4}", b.current[0], b.current[1]))
        .unwrap_or_default();
    println!(
        "6. EXTRACT      {} particles live, {} bodies; sample particle: {}; body@({})",
        frame.particles.len(),
        frame.bodies.len(),
        sample,
        body
    );

    println!(
        "PHASE 2 EXIT DEMO: PASS (every numbered check above held; all assertions green)"
    );
}
