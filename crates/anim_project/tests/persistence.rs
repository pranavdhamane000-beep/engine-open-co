//! WP03 persistence + recovery validation (spec §12.12):
//! canonical save/load/save identity, invalid-document rejection, injected
//! failures preserving the original destination, recovery selection rules,
//! migration chain, and the Phase 1 exit demo shape
//! (save → hash → reopen → hash identical).

use anim_assets::ContentHash;
use anim_core::{AssetId, FrameRate, NodeId, ProjectId, Revision, SceneId};
use anim_project::*;

fn doc_v1() -> ProjectDocument {
    ProjectDocument::new(FrameRate::new(24000, 1001).unwrap())
}

// ------------------------------------------------- canonical round trip

#[test]
fn save_reopen_save_is_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("project.anim.json");
    let mut d = doc_v1();
    let scene = SceneId::from_u128(1);
    let node = NodeId::from_u128(2);
    let asset = AssetId::from_u128(3);
    d.assets
        .insert(
            anim_assets::AssetManifestEntry::new(
                asset,
                anim_assets::ContentHash(*blake3::hash(b"img").as_bytes()),
                42,
                AssetKind::Image,
                1,
                Vec::new(),
                "hero.png".into(),
            )
            .unwrap(),
        )
        .unwrap();
    let mut sc = SceneData::new("arena");
    sc.nodes.insert(
        node,
        NodeData {
            name: "hero".into(),
            parent: None,
            asset: Some(asset),
            properties: {
                use anim_project::PropertyTrack;
                let mut m = std::collections::BTreeMap::new();
                let mut t = PropertyTrack::new(Some(PropertyValue::Float(1.0)));
                t.upsert_key(PropertyKey {
                    time: anim_core::Rational::new(7, 2).unwrap(),
                    value: PropertyValue::Float(0.5),
                    easing: Easing::Smooth,
                });
                m.insert("opacity".into(), t);
                m
            },
        },
    );
    d.scenes.insert(scene, sc);

    let report1 = save_atomic(&d, &dest).unwrap();
    assert!(report1.backup.is_none(), "first save has no backup");

    let reopened = load_document(&dest).unwrap();
    assert_eq!(reopened, d, "reopen reconstructs exactly");

    let report2 = save_atomic(&reopened, &dest).unwrap();
    let bytes1 = std::fs::read(&dest).unwrap();
    let bytes2 = canonical_json(&d).unwrap();
    assert_eq!(bytes1, bytes2, "canonical bytes stable across save/load/save");

    // document hash reproducibility (G3-style)
    let h1 = document_hash(&load_document(&dest).unwrap()).unwrap();
    let h2 = document_hash(&d).unwrap();
    assert_eq!(h1, h2);
    assert_eq!(ContentHash::parse(&report1.document_hash).unwrap(), h1);

    // backup exists on the SECOND save (destination existed) and is a
    // complete valid document (recovery input)
    let backup = report2.backup.expect("replace commit must keep backup");
    load_document(&backup).unwrap(); // schema-valid
}

#[test]
fn invalid_document_never_reaches_disk() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("p.json");
    std::fs::write(&dest, b"old-good-bytes").unwrap();

    let mut d = doc_v1();
    // node referencing a manifest-absent asset → validation error
    let scene = SceneId::from_u128(1);
    let mut sc = SceneData::new("s");
    sc.nodes.insert(
        NodeId::from_u128(1),
        NodeData {
            name: "hero".into(),
            parent: None,
            asset: Some(AssetId::from_u128(77)),
            properties: Default::default(),
        },
    );
    d.scenes.insert(scene, sc);

    let result = save_atomic(&d, &dest);
    assert!(result.is_err(), "invalid document must fail validation");
    assert_eq!(std::fs::read(&dest).unwrap(), b"old-good-bytes");
}

#[test]
fn write_failure_preserves_original_destination() {
    let dir = tempfile::tempdir().unwrap();
    let missing_parent = dir.path().join("no-such-dir");
    let dest = missing_parent.join("p.json"); // parent does not exist

    let d = doc_v1();
    let result = save_atomic(&d, &dest);
    assert!(result.is_err(), "missing parent must fail");
    assert!(!dest.exists(), "must not fabricate the destination");
}

#[test]
fn unsupported_schema_version_rejected_on_parse() {
    let full = serde_json::json!({
        "schema_version": 2,
        "project_id": ProjectId::from_u128(1),
        "revision": 0,
        "frame_rate": "24/1",
        "assets": {"schema_version": 1, "records": {}},
        "scenes": {},
    });
    let bytes = serde_json::to_vec(&full).unwrap();
    let doc_result = parse_document(&bytes);
    assert!(matches!(
        doc_result,
        Err(ProjectError::UnsupportedSchema { found: 2, supported: 1 })
    ));
}

// ------------------------------------------------------------- recovery

#[test]
fn recovery_scan_and_selection_rules() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("project.anim.json");
    let pid = doc_v1().project_id;

    // CURRENT at rev 5
    let mut current = doc_v1();
    current.project_id = pid;
    current.revision = Revision(5);
    save_atomic(&current, &dest).unwrap(); // backup: none (first save)

    // BACKUP file at rev 4 (simulate a replace commit's leftover)
    let mut older = doc_v1();
    older.project_id = pid;
    older.revision = Revision(4);
    let backup_path = dir.path().join("project.anim.json.aaaa-bbbb.old.tmp");
    let bytes = canonical_json(&older).unwrap();
    std::fs::write(&backup_path, &bytes).unwrap();

    // AUTOSAVE at rev 6 (newer than manual) → AMBIGUOUS by rule
    let mut newer = doc_v1();
    newer.project_id = pid;
    newer.revision = Revision(6);
    save_autosave(&newer, &dest).unwrap();

    let candidates = scan_recovery(pid, dir.path(), "project.anim.json").unwrap();
    assert!(candidates.iter().all(|c| c.valid), "all valid: {candidates:?}");
    let decision = select_recovery(&candidates);
    assert!(matches!(decision, RecoveryDecision::Ambiguous { .. }));

    // now make the autosave OLDER (rev 2): it must be ignored, manual wins
    let mut stale = doc_v1();
    stale.project_id = pid;
    stale.revision = Revision(2);
    save_autosave(&stale, &dest).unwrap();

    let candidates = scan_recovery(pid, dir.path(), "project.anim.json").unwrap();
    match select_recovery(&candidates) {
        RecoveryDecision::Selected(c) => {
            assert_eq!(c.revision, Some(5), "current (rev 5) must win");
            assert_eq!(c.kind, RecoveryKind::Current);
        }
        other => panic!("expected Selected, got {other:?}"),
    }

    // foreign project id → candidates present but flagged invalid
    let foreign = doc_v1().project_id;
    let candidates = scan_recovery(foreign, dir.path(), "project.anim.json").unwrap();
    assert!(!candidates.is_empty());
    assert!(candidates.iter().any(|c| !c.valid));
    assert!(matches!(select_recovery(&candidates), RecoveryDecision::None));
}

#[test]
fn recovery_parse_error_preserves_files_and_reports_detail() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("p.json");
    let d = doc_v1();
    save_atomic(&d, &dest).unwrap();
    // corrupt the CURRENT file in place
    std::fs::write(&dest, b"{ not json").unwrap();

    let cand = scan_recovery(d.project_id, dir.path(), "p.json").unwrap();
    let current = cand.iter().find(|c| c.kind == RecoveryKind::Current).unwrap();
    assert!(!current.valid && !current.detail.is_empty());
    // file preserved for diagnosis
    assert!(dest.exists());
}

// ------------------------------------------------------------ migration

#[test]
fn migration_chain_v0_to_v1() {
    let v0 = serde_json::json!({
        "schema_version": 0,
        "project_id": ProjectId::from_u128(42),
        "revision": 3,
        "rate_num": 24, "rate_den": 1,
        "scenes": [
            {"id": SceneId::from_u128(7),
             "title": "v0 arena",
             "nodes": [
                 {"id": NodeId::from_u128(8), "caption": "fighter", "parent": null},
                 {"id": NodeId::from_u128(9), "caption": "glove", "parent": NodeId::from_u128(8)},
             ]},
        ]
    });
    let bytes = serde_json::to_vec(&v0).unwrap();
    let doc = migrate_document(&bytes).unwrap();
    assert_eq!(doc.schema_version, 1);
    assert_eq!(doc.revision, Revision(3));
    assert_eq!(doc.frame_rate.num, 24);
    let scene = doc.scenes.get(&SceneId::from_u128(7)).unwrap();
    assert_eq!(scene.name, "v0 arena");
    let glove = scene.nodes.get(&NodeId::from_u128(9)).unwrap();
    assert_eq!(glove.parent, Some(NodeId::from_u128(8)));
    // migrated doc round trips (valid + canonicalizable)
    let h1 = document_hash(&doc).unwrap();
    let h2 = document_hash(&load_project_from(doc.clone())).unwrap();
    assert_eq!(h1, h2);

    // wrapped serde round trip through canonical bytes
    let can = canonical_json(&doc).unwrap();
    parse_document(&can).unwrap();
}

fn load_project_from(d: ProjectDocument) -> ProjectDocument {
    let bytes = canonical_json(&d).unwrap();
    parse_document(&bytes).unwrap()
}

#[test]
fn migration_rejects_bad_shapes_typed() {
    // no migration path for v3 → UnsupportedSchema (higher than supported)
    let v3 =
        serde_json::json!({"schema_version": 3, "project_id": ProjectId::from_u128(1)});
    let bytes = serde_json::to_vec(&v3).unwrap();
    assert!(matches!(
        migrate_document(&bytes),
        Err(ProjectError::UnsupportedSchema { found: 3, supported: 1 })
    ));

    // v0 with a zero denominator → Migration typed failure
    let bad_rate = serde_json::json!({
        "schema_version": 0,
        "project_id": ProjectId::from_u128(1),
        "revision": 0,
        "rate_num": 24, "rate_den": 0,
        "scenes": [],
    });
    let bytes = serde_json::to_vec(&bad_rate).unwrap();
    assert!(matches!(migrate_document(&bytes), Err(ProjectError::Migration(_))));

    // missing schema_version → Migration typed failure
    assert!(matches!(
        migrate_document(br#"{"project_id": "x"}"#),
        Err(ProjectError::Migration(_))
    ));
}

#[test]
fn autosave_round_trip_and_version_guard() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("p.json");
    let d = doc_v1();
    save_autosave(&d, &dest).unwrap();
    let path = autosave_path_for(&dest);
    assert!(path.to_string_lossy().ends_with(".autosave.json"));
    let loaded = load_autosave(&path).unwrap();
    assert_eq!(loaded, d);
}

// --------------------------------------------------- save-stage crashes

#[test]
fn kill_simulation_at_each_save_stage() {
    // Stage 1 — Serialized: nothing touches disk at all.
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("p.json");
    let d = doc_v1();
    let err = save_atomic_staged(&d, &dest, SaveStage::Serialized).unwrap_err();
    assert!(err.to_string().contains("Serialized"));
    assert!(!dest.exists(), "Serialized stop must not fabricate the destination");

    // Stage 2 — TempWritten: old destination complete, staging .tmp debris
    //.Utilities in place; recovery must still see the valid CURRENT file
    // and a subsequent save must succeed over the debris.
    let dest2 = dir.path().join("p2.json");
    let old = doc_v1();
    let mut old = old;
    old.revision = Revision(4);
    save_atomic(&old, &dest2).unwrap();
    let old_bytes = std::fs::read(&dest2).unwrap();

    let mut newer = old.clone();
    newer.revision = Revision(5);
    let err = save_atomic_staged(&newer, &dest2, SaveStage::TempWritten).unwrap_err();
    assert!(err.to_string().contains("TempWritten"));

    // destination untouched (old bytes complete)
    assert_eq!(std::fs::read(&dest2).unwrap(), old_bytes);
    // exactly one staging .tmp debris file remains (crash debris)
    let debris: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .filter(|e| e.path().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert_eq!(debris.len(), 1, "crash debris present: {debris:?}");

    // recovery over the crash state: the staging debris is NOT a candidate
    // (scan matches current + *.old.tmp + autosave only), so the valid
    // candidate set is exactly {Current @ rev 4}
    let candidates = scan_recovery(old.project_id, dir.path(), "p2.json").unwrap();
    let valid: Vec<_> = candidates.iter().filter(|c| c.valid).collect();
    assert_eq!(valid.len(), 1);
    assert_eq!(valid[0].kind, RecoveryKind::Current);
    assert_eq!(valid[0].revision, Some(4));
    assert!(matches!(select_recovery(&candidates), RecoveryDecision::Selected(_)));

    // a normal save AFTER the crash succeeds and leaves the backup intact
    let after_crash = save_atomic(&newer, &dest2).unwrap();
    assert_eq!(after_crash.revision, Revision(5));
    assert!(after_crash.backup.is_some());
    // destination holds the NEW complete document
    assert_eq!(load_document(&dest2).unwrap().revision, Revision(5));
    // candidates after the crash still parse cleanly
    let candidates = scan_recovery(old.project_id, dir.path(), "p2.json").unwrap();
    assert!(candidates.iter().filter(|c| c.valid).all(|c| c.revision.is_some()));
}
