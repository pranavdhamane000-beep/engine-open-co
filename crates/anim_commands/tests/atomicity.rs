//! WP03 atomicity validation (spec §12.12): rollback from every command
//! index, stale-envelope rejection, exact duplicate handling, one revision
//! increment per batch, undo/redo equality, redo clearing, dry-run/apply
//! equivalence, canonical round trips, revision capacity, and registry
//! (JSON automation route) equivalence.

use anim_assets::*;
use anim_commands::command::Command;
use anim_commands::envelope::CommandEnvelope;
use anim_commands::registry;
use anim_commands::{CommandError, CommandOutcome, CommandProcessor, Notification};
use anim_core::{AssetId, FrameRate, NodeId, Rational, Revision, SceneId};
use anim_project::*;

fn doc() -> ProjectDocument {
    ProjectDocument::new(FrameRate::new(24, 1).unwrap())
}

// ------------------------------------------------------------ helpers

fn asset_entry(tag: u8, asset_id: AssetId) -> AssetManifestEntry {
    AssetManifestEntry::new(
        asset_id,
        ContentHash(*blake3::hash(&[tag]).as_bytes()),
        tag as u64,
        AssetKind::Image,
        1,
        Vec::new(),
        format!("asset-{tag}"),
    )
    .unwrap()
}

fn key(at: (i128, i128), value: f64) -> PropertyKey {
    PropertyKey {
        time: Rational::new(at.0, at.1).unwrap(),
        value: PropertyValue::Float(value),
        easing: Easing::Linear,
    }
}

fn apply_single(
    proc: &mut CommandProcessor,
    label: &str,
    cmds: Vec<Command>,
) -> CommandOutcome {
    let env = CommandEnvelope::new(proc.document(), label, cmds);
    proc.apply(env).unwrap()
}

// ------------------------------------------------- rollback from every index

#[test]
fn rollback_from_every_command_index() {
    let scene = SceneId::from_u128(100);

    // 4-command batch: scene, node-a, asset-good, then a BAD i-th command
    for bad_index in 0..5 {
        let mut p = CommandProcessor::new(doc());
        let scene_cmd = Command::UpsertScene { scene_id: scene, name: "s".into() };
        let node_a = NodeId::from_u128(1);
        let node_b = NodeId::from_u128(2);
        let asset_ok = asset_entry(1, AssetId::from_u128(7));
        let node_cmd = Command::UpsertNode {
            scene_id: scene,
            node_id: node_a,
            name: "a".into(),
            parent: None,
            asset: Some(AssetId::from_u128(7)),
        };
        let asset_cmd = Command::UpsertAsset { entry: asset_ok };

        // break the i-th command: missing parent / duplicate-different asset
        let bad = [
            Command::UpsertNode {
                scene_id: SceneId::from_u128(404), // missing scene
                node_id: node_b,
                name: "b".into(),
                parent: None,
                asset: None,
            },
            Command::UpsertNode {
                scene_id: scene,
                node_id: node_b,
                name: "b".into(),
                parent: Some(NodeId::from_u128(999)), // missing parent
                asset: None,
            },
            Command::UpsertAsset {
                // same id, different content than above → contract breach
                entry: asset_entry(2, AssetId::from_u128(7)),
            },
            Command::RemoveNode {
                scene_id: scene,
                node_id: node_b, // never created
            },
            Command::RemoveAsset { asset_id: AssetId::from_u128(999) },
        ];
        let mut batch = vec![scene_cmd, node_cmd, asset_cmd];
        // replace command #bad_index in the FULL ordered batch [scene, node,
        // asset, node-b] with the matching broken command
        batch.truncate(bad_index.min(3));
        batch.push(bad[bad_index].clone());

        let env = CommandEnvelope::new(p.document(), "broken", batch);
        let before_bytes = serde_json::to_vec(p.document()).unwrap();
        let before_rev = p.document().revision();
        let result = p.apply(env);

        assert!(result.is_err(), "bad index {bad_index} must reject");
        // document unchanged + revision unchanged (no batch prefix committed)
        assert_eq!(p.document().revision(), before_rev);
        let after_bytes = serde_json::to_vec(p.document()).unwrap();
        assert_eq!(before_bytes, after_bytes, "index {bad_index}");
        assert_eq!(p.undo_depth(), 0);
        assert!(p.take_notifications().is_empty());
    }
}

// ------------------------------------------------------- stale envelope

#[test]
fn stale_envelope_rejected_with_revision_conflict() {
    let mut p = CommandProcessor::new(doc());
    let scene = SceneId::from_u128(1);
    apply_single(
        &mut p,
        "one",
        vec![Command::UpsertScene { scene_id: scene, name: "a".into() }],
    );

    // capture the envelope while fresh, then advance the document
    let env = CommandEnvelope::new(
        p.document(),
        "stale",
        vec![Command::UpsertScene { scene_id: SceneId::from_u128(2), name: "b".into() }],
    );
    // DON'T apply; make progress through another path instead
    apply_single(&mut p, "two", vec![Command::SetFrameRate { num: 30, den: 1 }]);

    assert!(matches!(
        p.apply(env),
        Err(CommandError::RevisionConflict { expected: 1, actual: 2 })
    ));
}

// --------------------------------------------------- exact duplicates

#[test]
fn exact_duplicate_idempotent_but_modified_hash_rejected() {
    let mut p = CommandProcessor::new(doc());
    let scene = SceneId::from_u128(1);
    let mut env = CommandEnvelope::new(
        p.document(),
        "txn",
        vec![Command::UpsertScene { scene_id: scene, name: "a".into() }],
    );
    let outcome = p.apply(env.clone()).unwrap();

    // exact duplicate AFTER progress → original result returned as-is
    // (bypasses the revision conflict — replay path precedes it, spec §12.7)
    apply_single(&mut p, "later", vec![Command::SetFrameRate { num: 48, den: 1 }]);
    let replay = p.apply(env.clone()).unwrap();
    assert_eq!(replay.new_revision, outcome.new_revision);
    assert_eq!(replay.envelope_hash, outcome.envelope_hash);

    // SAME command id but MODIFIED payload → replay mismatch
    env.transaction_label = "tampered".into();
    assert!(matches!(p.apply(env), Err(CommandError::CommandReplayMismatch(_))));
}

// ------------------------------------------- one increment per batch

#[test]
fn one_revision_increment_per_batch() {
    let mut p = CommandProcessor::new(doc());
    let before = p.document().revision().as_u64();
    apply_single(
        &mut p,
        "batch of four",
        vec![
            Command::UpsertScene { scene_id: SceneId::from_u128(1), name: "a".into() },
            Command::UpsertScene { scene_id: SceneId::from_u128(2), name: "b".into() },
            Command::UpsertScene { scene_id: SceneId::from_u128(3), name: "c".into() },
            Command::SetFrameRate { num: 60, den: 1 },
        ],
    );
    assert_eq!(p.document().revision().as_u64(), before + 1);
}

// ------------------------------------------------------ undo / redo

#[test]
fn undo_redo_equality_and_redo_clearing() {
    // Undo commits a NEW revision (spec 12.6), so byte equality includes a
    // changed revision field; content equality is the exact-inverse proof.
    fn content(doc: &anim_project::ProjectDocument) -> String {
        let mut clone = doc.clone();
        clone.revision = Revision(0);
        serde_json::to_string(&clone).unwrap()
    }

    let mut p = CommandProcessor::new(doc());
    let scene = SceneId::from_u128(1);
    let original_content = content(p.document());

    apply_single(
        &mut p,
        "edit",
        vec![Command::UpsertScene { scene_id: scene, name: "arena".into() }],
    );
    // second edit for multi-level undo
    apply_single(
        &mut p,
        "edit2",
        vec![Command::UpsertScene {
            scene_id: SceneId::from_u128(2),
            name: "stage".into(),
        }],
    );
    let after_edits_content = content(p.document());

    // undo #2 → document == state after edit #1 (content-level)
    let _ = p.undo().unwrap();
    assert!(p.document().scenes.contains_key(&scene));
    assert!(!p.document().scenes.contains_key(&SceneId::from_u128(2)));

    // undo #1 → pre-edit content exactly
    p.undo().unwrap();
    assert_eq!(content(p.document()), original_content, "two undos = pre-edit content");

    // redo #1 → state after edit #1 again
    p.redo().unwrap();
    assert!(p.document().scenes.contains_key(&scene));
    assert!(!p.document().scenes.contains_key(&SceneId::from_u128(2)));

    // redo #2 → full state again
    p.redo().unwrap();
    assert_eq!(content(p.document()), after_edits_content);

    // NEW forward edit clears redo
    apply_single(&mut p, "new", vec![Command::SetFrameRate { num: 60, den: 1 }]);
    assert!(matches!(p.redo(), Err(CommandError::RedoEmpty)));
    assert_eq!(p.undo_depth(), 3);
}

#[test]
fn undo_empty_error_is_typed() {
    let mut p = CommandProcessor::new(doc());
    assert!(matches!(p.undo(), Err(CommandError::UndoEmpty)));
}

// --------------------------------------------------- dry-run equivalence

#[test]
fn dry_run_apply_equivalence_at_one_revision() {
    let mut p = CommandProcessor::new(doc());
    let scene = SceneId::from_u128(1);
    let env = CommandEnvelope::new(
        p.document(),
        "probe",
        vec![
            Command::UpsertScene { scene_id: scene, name: "a".into() },
            Command::UpsertNode {
                scene_id: scene,
                node_id: NodeId::from_u128(5),
                name: "hero".into(),
                parent: None,
                asset: None,
            },
            Command::SetNodeProperty {
                scene_id: scene,
                node_id: NodeId::from_u128(5),
                property: "opacity".into(),
                key: key((7, 1), 0.5),
            },
        ],
    );

    // dry-run: no mutation of any kind
    let start_bytes = serde_json::to_vec(p.document()).unwrap();
    let report = p.dry_run(&env).unwrap();
    assert_eq!(report.applied, 3);
    assert_eq!(report.resulting_revision.as_u64(), report.start_revision.as_u64() + 1);
    assert_eq!(p.document().revision().as_u64(), report.start_revision.as_u64());
    assert_eq!(serde_json::to_vec(p.document()).unwrap(), start_bytes);
    assert_eq!(p.undo_depth(), 0);

    // apply at the SAME revision succeeds identically
    let outcome = p.apply(env).unwrap();
    assert_eq!(outcome.new_revision, report.resulting_revision);

    // proof: the same dry-run would now fail with revision conflict
    let env2 = CommandEnvelope::new(p.document(), "probe2", vec![]);
    // empty batch check comes BEFORE revision check? NO — spec order puts
    // project/revision checks first; empty batch is checked in execution.
    // Dry-run of an empty batch at the CURRENT revision:
    let report2 = p.dry_run(&env2);
    assert!(matches!(report2, Err(CommandError::EmptyBatch)));
}

// -------------------------------------------------- capacity + overflow

#[test]
fn revision_overflow_is_typed_rejection() {
    let mut d = doc();
    d.revision = Revision(u64::MAX);
    let mut p = CommandProcessor::new(d);
    let env = CommandEnvelope::new(
        p.document(),
        "overflow",
        vec![Command::UpsertScene { scene_id: SceneId::from_u128(1), name: "a".into() }],
    );
    assert!(matches!(p.apply(env), Err(CommandError::RevisionOverflow)));
}

// --------------------------------------------------- registry JSON route

#[test]
fn registry_json_dispatch_matches_typedroute() {
    let scene = SceneId::from_u128(11);
    let node = NodeId::from_u128(12);

    // external automation JSON shape
    let envelope_json = serde_json::json!({
        "schema_version": 1,
        "project_id": doc().project_id,
        "command_id": anim_core::CommandId::new_v4(),
        "expected_scene_revision": 0,
        "transaction_label": "automation",
        "commands": [
            {"type": "upsert_scene", "scene_id": scene, "name": "json-scene"},
            {"type": "upsert_node", "scene_id": scene, "node_id": node,
             "name": "json-node", "parent": null, "asset": null},
            {"type": "set_node_property", "scene_id": scene, "node_id": node,
             "property": "opacity",
             "key": {"time": "1/2", "value": {"Float": 0.75}, "easing": "Linear"}},
            {"type": "set_frame_rate", "num": 30, "den": 1},
        ]
    });

    let env = registry::envelope_from_json(&envelope_json).unwrap();
    let mut p = CommandProcessor::new(doc());
    // project id from the FIXED test doc: rebuild envelope against it
    let env = CommandEnvelope { project_id: p.document().project_id, ..env };
    p.apply(env).unwrap();

    assert!(p.document().scenes.contains_key(&scene));
    let n = p.document().scenes[&scene].nodes.get(&node).unwrap();
    assert_eq!(n.properties["opacity"].keys[0].value, PropertyValue::Float(0.75));
    assert!(matches!(p.document().frame_rate, FrameRate { num: 30, den: 1 }));

    // unknown tag → UnknownCommand; unsupported schema → UnsupportedSchema
    let bad_tag = serde_json::json!({"type": "fly_to_mars"});
    assert!(matches!(
        registry::command_from_value(&bad_tag),
        Err(CommandError::UnknownCommand(_))
    ));
    let bad_version = serde_json::json!({"schema_version": 99});
    assert!(matches!(
        registry::envelope_from_json(&bad_version),
        Err(CommandError::UnsupportedSchema { found: 99, supported: 1 })
    ));
}

// ---------------------------------------------------- notifications

#[test]
fn notifications_published_after_commit_only() {
    let mut p = CommandProcessor::new(doc());
    // failure first: no notifications
    let bad = CommandEnvelope::new(
        p.document(),
        "bad",
        vec![Command::RemoveNode {
            scene_id: SceneId::from_u128(1),
            node_id: NodeId::from_u128(1),
        }],
    );
    assert!(p.apply(bad).is_err());
    assert!(p.take_notifications().is_empty());

    apply_single(
        &mut p,
        "good",
        vec![Command::UpsertScene { scene_id: SceneId::from_u128(1), name: "s".into() }],
    );
    let notes = p.take_notifications();
    assert_eq!(notes.len(), 1);
    assert!(matches!(notes[0], Notification::Applied { .. }));

    p.undo().unwrap();
    let notes = p.take_notifications();
    assert!(matches!(notes[0], Notification::Undone { .. }));
}

// ------------------------------------------------- full document integrity

#[test]
fn parent_cycle_rejected_via_commands() {
    let mut p = CommandProcessor::new(doc());
    let scene = SceneId::from_u128(1);
    let a = NodeId::from_u128(1);
    let b = NodeId::from_u128(2);
    apply_single(
        &mut p,
        "seed",
        vec![
            Command::UpsertScene { scene_id: scene, name: "s".into() },
            Command::UpsertNode {
                scene_id: scene,
                node_id: a,
                name: "a".into(),
                parent: None,
                asset: None,
            },
            Command::UpsertNode {
                scene_id: scene,
                node_id: b,
                name: "b".into(),
                parent: Some(a),
                asset: None,
            },
        ],
    );
    // A under B would close a cycle
    let env = CommandEnvelope::new(
        p.document(),
        "cycle",
        vec![Command::UpsertNode {
            scene_id: scene,
            node_id: a,
            name: "a".into(),
            parent: Some(b),
            asset: None,
        }],
    );
    assert!(matches!(p.apply(env), Err(CommandError::Validation(_))));

    // asset-reference-before-import is rejected
    let env2 = CommandEnvelope::new(
        p.document(),
        "ghost",
        vec![Command::UpsertNode {
            scene_id: scene,
            node_id: a,
            name: "a".into(),
            parent: None,
            asset: Some(AssetId::from_u128(999)),
        }],
    );
    assert!(matches!(p.apply(env2), Err(CommandError::Validation(_))));
}

#[test]
fn remove_rules_protect_dangling_references() {
    let mut p = CommandProcessor::new(doc());
    let scene = SceneId::from_u128(1);
    let a = NodeId::from_u128(1);
    let b = NodeId::from_u128(2);
    let asset = AssetId::from_u128(9);
    apply_single(
        &mut p,
        "seed",
        vec![
            Command::UpsertScene { scene_id: scene, name: "s".into() },
            Command::UpsertAsset { entry: asset_entry(3, asset) },
            Command::UpsertNode {
                scene_id: scene,
                node_id: a,
                name: "a".into(),
                parent: None,
                asset: Some(asset),
            },
            Command::UpsertNode {
                scene_id: scene,
                node_id: b,
                name: "b".into(),
                parent: Some(a),
                asset: None,
            },
        ],
    );
    // removing a parented node with children → rejected
    let env = CommandEnvelope::new(
        p.document(),
        "kill-parent",
        vec![Command::RemoveNode { scene_id: scene, node_id: a }],
    );
    assert!(matches!(p.apply(env), Err(CommandError::Validation(_))));

    // asset removal while referenced → rejected
    let env2 = CommandEnvelope::new(
        p.document(),
        "kill-asset",
        vec![Command::RemoveAsset { asset_id: asset }],
    );
    assert!(matches!(p.apply(env2), Err(CommandError::Validation(_))));

    // leaf removal restores verbatim through undo (NodeData exact)
    fn content(p_doc: &mut CommandProcessor) -> String {
        let mut c = p_doc.document().clone();
        c.revision = Revision(0);
        serde_json::to_string(&c).unwrap()
    }
    let content_before = content(&mut p);
    apply_single(
        &mut p,
        "remove-leaf",
        vec![Command::RemoveNode { scene_id: scene, node_id: b }],
    );
    p.undo().unwrap();
    assert_eq!(content(&mut p), content_before);
}
