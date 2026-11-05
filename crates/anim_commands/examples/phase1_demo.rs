//! PHASE 1 EXIT DEMO (spec gate): build a project through commands → save
//! atomically → hash → reopen → hash again → the two hashes MUST be
//! identical, proving the persistence substrate (WP01 time/IDs + WP02
//! assets + WP03 commands/undo/save) round trips byte-exactly.
//!
//! Run: cargo run -p anim_project --example phase1_demo

use anim_assets::{AssetKind, AssetManifestEntry};
use anim_commands::command::Command;
use anim_commands::envelope::CommandEnvelope;
use anim_commands::CommandProcessor;
use anim_core::{AssetId, FrameRate, NodeId, SceneId};
use anim_project::{document_hash, load_document, save_atomic};

fn main() {
    let dir = std::env::temp_dir().join("anim_phase1_demo");
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("phase1.project.json");

    // 1. fresh 24fps document, edited ONLY through the command processor
    let doc = anim_project::ProjectDocument::new(FrameRate::new(24, 1).unwrap());
    let scene = SceneId::from_u128(0xA1);
    let hero = NodeId::from_u128(0xB2);
    let shader = AssetId::from_u128(0xC3);

    let asset = AssetManifestEntry::new(
        shader,
        anim_assets::ContentHash(*blake3::hash(b"phase1-hero-png").as_bytes()),
        1_234,
        AssetKind::Image,
        1,
        Vec::new(),
        "hero.png".into(),
    )
    .unwrap();

    let mut processor = CommandProcessor::new(doc);
    let steps: &[(&str, Vec<Command>)] = &[
        (
            "seed scene",
            vec![Command::UpsertScene { scene_id: scene, name: "arena".into() }],
        ),
        ("import asset", vec![Command::UpsertAsset { entry: asset }]),
        (
            "hero node",
            vec![Command::UpsertNode {
                scene_id: scene,
                node_id: hero,
                name: "hero".into(),
                parent: None,
                asset: Some(shader),
            }],
        ),
        (
            "animate opacity",
            vec![Command::SetNodeProperty {
                scene_id: scene,
                node_id: hero,
                property: "opacity".into(),
                key: anim_project::PropertyKey {
                    time: anim_core::Rational::new(7, 2).unwrap(),
                    value: anim_project::PropertyValue::Float(0.5),
                    easing: anim_project::Easing::Smooth,
                },
            }],
        ),
    ];
    for (label, commands) in steps {
        let envelope = CommandEnvelope::new(processor.document(), *label, commands.clone());
        let outcome = processor.apply(envelope).unwrap();
        println!("applied [{label}] → revision {}", outcome.new_revision.as_u64());
    }

    // one undo/redo cycle for the history layer
    processor.undo().unwrap();
    processor.redo().unwrap();
    assert_eq!(processor.undo_depth(), 4);

    // 2. atomic save → hash
    let document = processor.document().clone();
    let report = save_atomic(&document, &dest).unwrap();
    println!(
        "saved → revision {} hash {}",
        report.revision.as_u64(),
        &report.document_hash[..16]
    );

    // 3. reopen → hash again
    let reopened = load_document(&dest).unwrap();
    let hash_before = document_hash(&document).unwrap();
    let hash_after = document_hash(&reopened).unwrap();

    // 4. THE GATE: hash-identical round trip
    assert_eq!(hash_before, hash_after, "Phase 1 exit: reopen must be hash-identical");
    assert_eq!(document.revision, reopened.revision);
    println!(
        "PHASE 1 EXIT DEMO PASS ✓ save→hash→reopen→hash identical: {}",
        hash_before.full_hex()
    );
}
