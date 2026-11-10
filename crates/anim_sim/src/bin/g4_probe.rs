//! G4 PROBE (spec §14.12): a fresh process loads a saved document, runs
//! the simulation forward with a FIXED cadence (7, cap 8), then seeks
//! (interleaved, non-monotone) to a FIXED target list, printing one
//! "tick N hash <64hex>" line per target. The integration test spawns
//! THIS BINARY twice; identical stdout = two fresh processes agree
//! byte-for-byte through seek + checkpoint + restore paths.
//!
//! args: <project.json path> <scene_id uuid>

use anim_sim::{seek, SimTick, SimulationContext};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        eprintln!("usage: g4_probe <project.json> <scene_id>");
        std::process::exit(2);
    }
    let doc = anim_project::load_document(std::path::Path::new(&args[0]))
        .expect("parse document");
    let scene_id: anim_core::SceneId = args[1].parse().expect("scene id");

    // Fixed plan, pinned byte-for-byte (any edit here is a G4 change).
    // Target 20,000 ticks = 10,000 frames at 24 fps × 2 ticks/frame — the
    // spec §14.12 full-horizon acceptance figure.
    let targets: [i64; 7] = [0, 20_000, 1, 137, 10_001, 15_977, 20_000];
    let mut context = SimulationContext::build(doc, scene_id, 7, 8).expect("context");
    for t in targets {
        let state = seek(&mut context, SimTick(t), None).expect("seek");
        let hash = state.canonical_hash().expect("state hash");
        println!("tick {t} hash {}", hash.full_hex());
    }
}
