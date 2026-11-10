//! G4 cross-process gate (spec §14.12): spawn the `g4_probe` binary in
//! TWO fresh processes against the SAME saved document; their stdout
//! (state hashes through seek + checkpoint + restore paths) must be
//! byte-identical. Mirrors the g3_probe pattern (WP04).

use std::process::Command;

fn fixture_document() -> (anim_project::ProjectDocument, anim_core::SceneId) {
    let mut doc =
        anim_project::ProjectDocument::new(anim_core::FrameRate::new(24, 1).unwrap());
    doc.simulation = anim_core::SimulationSettings {
        ticks_per_frame: 2,
        project_seed: [42u8; 32],
        gravity: [0.0, -9.0, 0.0],
        solver_iterations: 8,
    };
    let scene_id = anim_core::SceneId::new_v4();
    let mut scene = anim_project::SceneData::new("g4_fixture");
    let a = anim_core::NodeId::new_v4();
    let b = anim_core::NodeId::new_v4();
    scene.nodes.insert(
        a,
        anim_project::NodeData {
            name: "e0".into(),
            parent: None,
            asset: None,
            properties: Default::default(),
        },
    );
    scene.nodes.insert(
        b,
        anim_project::NodeData {
            name: "e1".into(),
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
    doc.scenes.insert(scene_id, scene);
    (doc, scene_id)
}

#[test]
fn two_fresh_processes_seeking_agree_byte_for_byte() {
    let (doc, scene_id) = fixture_document();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("g4_fixture.json");
    anim_project::save_atomic(&doc, &path).expect("save document");

    let exe = env!("CARGO_BIN_EXE_g4_probe");
    let first = Command::new(exe)
        .arg(&path)
        .arg(scene_id.to_string())
        .output()
        .expect("spawn probe once");
    let second = Command::new(exe)
        .arg(&path)
        .arg(scene_id.to_string())
        .output()
        .expect("spawn probe twice");

    assert!(
        first.status.success(),
        "probe 1 failed: {:?}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        second.status.success(),
        "probe 2 failed: {:?}",
        String::from_utf8_lossy(&second.stderr)
    );

    let out1 = String::from_utf8(first.stdout.clone()).unwrap();
    let out2 = String::from_utf8(second.stdout.clone()).unwrap();

    assert_eq!(out1.lines().count(), 7, "plan prints 7 lines; got:\n{out1}");
    assert_eq!(out1, out2, "two fresh processes diverged");

    // sanity: the prints reference the pinned targets, and tick 1 vs 0
    // differ (state actually evolves)
    let line_at =
        |tick: i64| out1.lines().find(|l| l.starts_with(&format!("tick {tick} "))).unwrap();
    assert_ne!(line_at(0), line_at(1), "tick 1 must evolve past tick 0");
}
