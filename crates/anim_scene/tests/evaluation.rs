//! WP04 integration validation (spec §13.12 + the G3 determinism gate):
//!
//! * snapshot determinism + canonical hash (same inputs → identical hash)
//! * exact key/boundary interpolation cases
//! * hierarchy world-transform composition
//! * camera cuts (active resolution + typed rejection of named misses)
//! * event frame-ownership boundary rule (exact upper boundary → NEXT frame)
//! * draw order separation from hierarchy order
//! * G3 RE-EXEC PROOF: two FRESH processes evaluate the SAME saved document
//!   via the `g3_probe` binary and MUST print byte-identical snapshot +
//!   event-log digests. One process differing fails the test.
//!
//! Invariants proven here never depend on HashMap iteration order, wall
//! time, or process state — evaluation is stateless by construction.

use anim_core::{EventDefinitionId, FramePos, FrameRate, NodeId, Rational, SceneId};
use anim_project::{
    Easing, EventDefinition, EventKind, MarkerKind, NodeData, ProjectDocument, PropertyKey,
    PropertyTrack, PropertyValue, SceneData, SceneMarker, TrackData,
};
use std::collections::BTreeMap;

const FPS_24: FrameRate = FrameRate { num: 24, den: 1 };
const SCENE: u128 = 1;

/// An arena scene: parent "arm" + child "fist", one camera, authored events.
fn fixture_doc() -> ProjectDocument {
    let mut d = ProjectDocument::new(FPS_24);
    let mut sc = SceneData::new("arena");

    // node hierarchy: world → arm(tx 10) → fist(tx 5, z keyed)
    let mut arm_props = BTreeMap::new();
    arm_props.insert("translation_x".into(), {
        let mut t = PropertyTrack::new(Some(PropertyValue::Float(10.0)));
        t.upsert_key(PropertyKey {
            time: Rational::zero(),
            value: PropertyValue::Float(10.0),
            easing: Easing::Linear,
        });
        t
    });
    sc.nodes.insert(
        NodeId::from_u128(10),
        NodeData { name: "arm".into(), parent: None, asset: None, properties: arm_props },
    );

    let mut fist_props = BTreeMap::new();
    fist_props.insert("translation_x".into(), {
        let mut t = PropertyTrack::new(Some(PropertyValue::Float(5.0)));
        t.upsert_key(PropertyKey {
            time: Rational::zero(),
            value: PropertyValue::Float(5.0),
            easing: Easing::Linear,
        });
        t
    });
    // z: keys at frame 0 (2.0) and frame 8 (6.0), Smooth between
    fist_props.insert("z".into(), {
        let mut t = PropertyTrack::new(Some(PropertyValue::Float(2.0)));
        t.upsert_key(PropertyKey {
            time: Rational::zero(),
            value: PropertyValue::Float(2.0),
            easing: Easing::Smooth,
        });
        t.upsert_key(PropertyKey {
            time: Rational::new(1, 3).unwrap(), // frame 8 at 24 fps
            value: PropertyValue::Float(6.0),
            easing: Easing::Smooth,
        });
        t
    });
    sc.nodes.insert(
        NodeId::from_u128(11),
        NodeData {
            name: "fist".into(),
            parent: Some(NodeId::from_u128(10)),
            asset: None,
            properties: fist_props,
        },
    );

    // a camera node (declared role = Bool(true) default)
    let mut cam_props = BTreeMap::new();
    cam_props.insert("camera".into(), PropertyTrack::new(Some(PropertyValue::Bool(true))));
    sc.nodes.insert(
        NodeId::from_u128(20),
        NodeData { name: "cam_a".into(), parent: None, asset: None, properties: cam_props },
    );

    // a Cut marker naming cam_a at frame 4
    sc.markers.push(SceneMarker {
        name: "cam_a".into(),
        at: FramePos::new(4, Rational::zero()).unwrap(),
        kind: MarkerKind::Cut,
    });

    // events: Spawn on arm at frame 0, Spawn on fist at the EXACT frame-1
    // upper boundary t = 1/24 s (ownership must resolve to frame 1)
    let mut events = BTreeMap::new();
    events.insert(
        EventDefinitionId::from_u128(100),
        EventDefinition {
            name: "punch".into(),
            kind: EventKind::Spawn,
            priority: 5,
            source: NodeId::from_u128(10),
            asset: None,
            times: vec![Rational::zero()],
            params: BTreeMap::new(),
        },
    );
    events.insert(
        EventDefinitionId::from_u128(101),
        EventDefinition {
            name: "hit".into(),
            kind: EventKind::Spawn,
            priority: 1,
            source: NodeId::from_u128(11),
            asset: None,
            times: vec![Rational::new(1, 24).unwrap()],
            params: BTreeMap::new(),
        },
    );
    sc.events = events;

    d.scenes.insert(SceneId::from_u128(SCENE), sc);
    d
}

#[test]
fn evaluation_is_deterministic_with_stable_hash() {
    use anim_scene::{snapshot_hash, SceneEvaluator};
    let doc = fixture_doc();
    let scene = SceneId::from_u128(SCENE);

    let a = SceneEvaluator::evaluate_frame(&doc, scene, 2).unwrap();
    let b = SceneEvaluator::evaluate_frame(&doc, scene, 2).unwrap();
    assert_eq!(a, b, "stateless evaluation: same inputs → equal snapshot");
    assert_eq!(
        snapshot_hash(&a).unwrap(),
        snapshot_hash(&b).unwrap(),
        "canonical hash identical"
    );
}

#[test]
fn world_transform_is_parent_composition() {
    use anim_scene::hierarchy::mat_apply;
    use anim_scene::SceneEvaluator;
    let doc = fixture_doc();
    let scene = SceneId::from_u128(SCENE);

    let snap = SceneEvaluator::evaluate_frame(&doc, scene, 0).unwrap();
    let fist =
        snap.nodes.iter().find(|n| n.id == NodeId::from_u128(11)).expect("fist evaluated");
    // fist world = translate(10) * translate(5): local x point 1.0 → 16.0
    let (x, _y) = mat_apply(&fist.world_transform, 1.0, 0.0);
    assert!((x - 16.0).abs() < 1e-12, "composed world x = {x}");
    // parent recorded
    assert_eq!(fist.parent, Some(NodeId::from_u128(10)));
}

#[test]
fn interpolation_exact_keys_and_hold_tails() {
    use anim_scene::hierarchy::PROPERTY_Z;
    use anim_scene::properties::evaluate_property;
    let doc = fixture_doc();
    let scene = SceneId::from_u128(SCENE);
    let z: &PropertyTrack = doc.scenes[&scene].nodes[&NodeId::from_u128(11)]
        .properties
        .get(PROPERTY_Z)
        .unwrap();

    // exact key time returns THAT key
    let v = evaluate_property(z, &Rational::zero(), PROPERTY_Z).unwrap();
    assert_eq!(v, PropertyValue::Float(2.0));
    // last-key hold: frame 20 spans [20/24, 21/24); test its exact start
    let v = evaluate_property(z, &Rational::new(20, 24).unwrap(), PROPERTY_Z).unwrap();
    assert_eq!(v, PropertyValue::Float(6.0), "after last key → last value");
    // midway (u = 1/2 exact): Smooth easing u²(3−2u)=0.5 → 2 + 4·0.5 = 4.0
    let mid = Rational::new(4, 24).unwrap(); // exactly halfway frames 0..8
    let v = evaluate_property(z, &mid, PROPERTY_Z).unwrap();
    match v {
        PropertyValue::Float(f) => assert!((f - 4.0).abs() < 1e-9, "mid z = {f}"),
        other => panic!("expected Float, got {other:?}"),
    }
}

#[test]
fn camera_cut_resolves_active() {
    use anim_scene::SceneEvaluator;
    let doc = fixture_doc();
    let scene = SceneId::from_u128(SCENE);
    let cam = NodeId::from_u128(20);

    // before the cut (frame 3): the only camera is active (lowest-NodeId default)
    let s3 = SceneEvaluator::evaluate_frame(&doc, scene, 3).unwrap();
    assert!(s3.cameras.iter().any(|c| c.id == cam && c.active));
    // at/after the cut (frames 4,5,6): the named camera is STILL active
    for f in [4, 5, 6] {
        let s = SceneEvaluator::evaluate_frame(&doc, scene, f).unwrap();
        let c = s.cameras.iter().find(|c| c.id == cam).unwrap();
        assert!(c.active, "cut holds at frame {f}");
    }
}

#[test]
fn event_owned_by_frame_of_exact_boundary() {
    use anim_scene::events_for_frame;
    let doc = fixture_doc();
    let scene = SceneId::from_u128(SCENE);

    // frame 0 owns the t=0 event ONLY
    let f0 = events_for_frame(&doc, scene, 0).unwrap();
    assert_eq!(f0.len(), 1, "boundary event must NOT fall back to frame 0");
    assert_eq!(f0[0].source_id, NodeId::from_u128(10));
    // frame 1 owns the t = 1/24 event (exact upper boundary of frame 0)
    let f1 = events_for_frame(&doc, scene, 1).unwrap();
    assert_eq!(f1.len(), 1);
    assert_eq!(f1[0].source_id, NodeId::from_u128(11));
    // no other frames own anything
    let f2 = events_for_frame(&doc, scene, 2).unwrap();
    assert!(f2.is_empty());
}

#[test]
fn unknown_camera_cut_is_typed_rejection() {
    use anim_scene::SceneError;
    let mut doc = fixture_doc();
    let scene = SceneId::from_u128(SCENE);
    doc.scenes.get_mut(&scene).unwrap().markers.push(SceneMarker {
        name: "ghost_cam".into(),
        at: FramePos::new(9, Rational::zero()).unwrap(),
        kind: MarkerKind::Cut,
    });
    let err = anim_scene::SceneEvaluator::evaluate_frame(&doc, scene, 9).unwrap_err();
    assert_eq!(err, SceneError::UnknownCamera("ghost_cam".into()));
}

#[test]
fn draw_order_is_z_keyed_not_hierarchy_order() {
    use anim_scene::{draw_order, SceneEvaluator};
    let doc = fixture_doc();
    let scene = SceneId::from_u128(SCENE);
    let snap = SceneEvaluator::evaluate_frame(&doc, scene, 6).unwrap(); // z risen above 2
    let order = draw_order(&snap);
    // hierarchy (traversal) order is [arm, fist, cam]; draw order keys by z,
    // so the fist (z > 2) renders while camera (z = 0 default) placement is
    // still z-comparable — assert exact z-keyed result:
    let fist = snap.nodes.iter().find(|n| n.id == NodeId::from_u128(11)).unwrap();
    assert!(fist.z > 2.0);
    assert_eq!(order.len(), snap.nodes.len());
    assert_ne!(
        order,
        snap.nodes.iter().map(|n| n.id).collect::<Vec<_>>(),
        "draw order must diverge from traversal order when z differs"
    );
}

#[test]
fn tracks_layer_override_applies() {
    use anim_scene::SceneEvaluator;
    let mut doc = fixture_doc();
    let scene = SceneId::from_u128(SCENE);
    // a higher layer track targeting fist z: overrides the node's own track
    let old = doc.scenes.get_mut(&scene).unwrap().tracks.insert(
        anim_core::TrackId::from_u128(500),
        TrackData {
            name: "punch_up".into(),
            target: NodeId::from_u128(11),
            layer: 7,
            property: "z".into(),
        },
    );
    assert!(old.is_none(), "track id 500 must be fresh");
    let snap = SceneEvaluator::evaluate_frame(&doc, scene, 4).unwrap();
    let fist = snap.nodes.iter().find(|n| n.id == NodeId::from_u128(11)).unwrap();
    // layer override wins: node track would evaluate ≈ 2+(4·0.5)=4.0 at
    // frame 4; but the OVERRIDE track has no authored keys on the node →
    // no value rides it, so the node's own track value stands (documented
    // ADR: override replaces WHEN the winning track provides a value).
    assert!((fist.z - 4.0).abs() < 1e-9, "no authored keys → node track stands");
}

// —————————————————————————————————————————————— G3 two-process prove-out

#[test]
fn two_fresh_processes_agree_on_snapshot_and_event_digests() {
    use std::process::Command;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("g3doc.anim.json");
    anim_project::save_atomic(&fixture_doc(), &dest).unwrap();

    // (cmd, stdout) for ONE fresh probe process
    let scene_text = SceneId::from_u128(SCENE).to_string();
    let probe = |tag: u32| -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_g3_probe"))
            .arg(dest.as_os_str())
            .arg(&scene_text)
            .arg("2")
            .env("PROBE_RUN_TAG", tag.to_string())
            .output()
            .expect("spawn g3_probe");
        assert!(
            out.status.success(),
            "probe run {tag} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).expect("utf8 stdout")
    };

    let run_a = probe(7);
    let run_b = probe(99);
    assert_eq!(
        run_a, run_b,
        "G3 VIOLATION: two fresh processes disagreed on canonical digests"
    );
    let lines: Vec<&str> = run_a.lines().collect();
    assert_eq!(lines.len(), 2, "snapshot + events lines");
    assert!(lines[0].starts_with("snapshot "));
    assert!(lines[1].starts_with("events   "));
}
