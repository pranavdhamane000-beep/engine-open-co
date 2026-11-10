//! G3 PROBE (spec §13.12): a fresh process evaluates one (document, scene,
//! frame) from a saved document file and prints the canonical snapshot
//! hash + event-log digest. The integration test spawns THIS BINARY twice;
//! two identical output lines = two fresh processes agree byte-for-byte.
//!
//! args: <project.json path> <scene_id uuid> <frame i64>

use anim_scene::{events_hash, snapshot_hash, SceneEvaluator};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        eprintln!("usage: g3_probe <project.json> <scene_id> <frame>");
        std::process::exit(2);
    }
    let doc = anim_project::load_document(std::path::Path::new(&args[0]))
        .expect("parse document");
    let scene_id: anim_core::SceneId = args[1].parse().expect("scene id");
    let frame: i64 = args[2].parse().expect("frame");

    let snapshot = SceneEvaluator::evaluate_frame(&doc, scene_id, frame).expect("evaluate");
    let snap = snapshot_hash(&snapshot).expect("snapshot hash");
    let ev = events_hash(&snapshot.events).expect("event hash");
    println!("snapshot {}", snap.full_hex());
    println!("events   {}", ev.full_hex());
}
