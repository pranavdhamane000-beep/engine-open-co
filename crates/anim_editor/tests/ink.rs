//! G-WP10 ink tests: duplicate events, exactly-once commit, history
//! ordering, cancellation, eraser and excessive points (spec 19.12).

use anim_commands::{Command as SetupCommand, CommandProcessor};
use anim_core::{FrameRate, NodeId, SceneId};
use anim_editor::ink::PointerEventKind;
use anim_editor::{
    EditorSession, InkEffect, InkEngine, PointerEvent, RawPoint, SnapshotReader,
    StrokeSetup, Tool,
};
use anim_project::ProjectDocument;

/// A pointer event at client coords with sensible defaults.
fn pe(kind: PointerEventKind, x: f32, y: f32, timestamp_us: u64) -> PointerEvent {
    PointerEvent {
        kind,
        pointer_id: 7,
        is_primary: true,
        is_eraser: false,
        is_mouse_synthesized: false,
        client_x: x,
        client_y: y,
        pressure: 0.5,
        tilt_x: 0,
        tilt_y: 0,
        timestamp_us,
        history: Vec::new(),
    }
}

fn setup(scale: f32, offset: (f32, f32)) -> StrokeSetup {
    StrokeSetup {
        view_scale: scale,
        view_offset_x: offset.0,
        view_offset_y: offset.1,
        scene_id: SceneId::new_v4(),
        node_id: NodeId::new_v4(),
        key_time_num: 12,
    }
}

/// Engine-level: direct InkEngine, explicit setup at pointer-down.
#[test]
fn ordered_events_commit_exactly_once() {
    let mut engine = InkEngine::new(Tool::Pen);
    let st = setup(2.0, (10.0, 5.0));

    let down = pe(PointerEventKind::Down, 0.0, 0.0, 0);
    let effect = engine.handle_pointer(down, Some(&st)).unwrap();
    assert!(matches!(effect, InkEffect::Captured { .. }));
    assert_eq!(engine.active_state(), anim_editor::StrokeState::Capturing);

    engine.handle_pointer(pe(PointerEventKind::Update, 1.0, 1.0, 8_000), None).unwrap();
    engine.handle_pointer(pe(PointerEventKind::Update, 2.0, 2.0, 16_000), None).unwrap();
    let stroke = engine.active_stroke().unwrap();
    assert_eq!(stroke.points.len(), 3);

    // Frozen transform applied: client (1,1) at scale 2 offset (10,5).
    assert_eq!(stroke.points[1].scene_x, 12.0);
    assert_eq!(stroke.points[1].scene_y, 7.0);

    let up = pe(PointerEventKind::Up, 3.0, 3.0, 24_000);
    let up_effect = engine.handle_pointer(up, None).unwrap();
    let (stroke_id, command_id) = match up_effect {
        InkEffect::StrokeUp { stroke_id, command_id } => (stroke_id, command_id),
        other => panic!("expected StrokeUp, got {other:?}"),
    };
    assert_eq!(engine.active_state(), anim_editor::StrokeState::CommitPending);

    // DUPLICATE UP on CommitPending: never commits again, points unchanged.
    let before = engine.active_stroke().unwrap().points.len();
    let dup =
        engine.handle_pointer(pe(PointerEventKind::Up, 3.0, 3.0, 25_000), None).unwrap();
    assert_eq!(dup, InkEffect::StrokeUp { stroke_id, command_id });
    assert_eq!(engine.active_stroke().unwrap().points.len(), before);

    // Update events at CommitPending mutate nothing.
    engine.handle_pointer(pe(PointerEventKind::Update, 4.0, 4.0, 26_000), None).unwrap();
    assert_eq!(engine.active_stroke().unwrap().points.len(), before);

    // Exactly-once finalize.
    let effect = engine.mark_committed(command_id).unwrap();
    assert_eq!(
        effect,
        InkEffect::Committed { command_id },
        "engine reports committed once"
    );
    assert_eq!(engine.active_state(), anim_editor::StrokeState::Idle);
    assert_eq!(engine.last_committed_command_id(), Some(command_id));
    assert!(engine.pending_commit().is_none());
}

#[test]
fn pointer_and_mouse_synthesis_never_commits() {
    let mut engine = InkEngine::new(Tool::Pen);
    let st = setup(1.0, (0.0, 0.0));

    // Synthesized mouse down is ignored while the pen tool owns input.
    let mut mouse_down = pe(PointerEventKind::Down, 0.0, 0.0, 0);
    mouse_down.is_mouse_synthesized = true;
    assert_eq!(engine.handle_pointer(mouse_down, Some(&st)).unwrap(), InkEffect::Ignored);
    assert_eq!(engine.active_state(), anim_editor::StrokeState::Idle);

    // Non-primary contact ignored.
    let mut secondary = pe(PointerEventKind::Down, 0.0, 0.0, 0);
    secondary.is_primary = false;
    assert_eq!(engine.handle_pointer(secondary, Some(&st)).unwrap(), InkEffect::Ignored);

    // Start a stroke, then a synthesized mouse up cannot terminate it.
    engine.handle_pointer(pe(PointerEventKind::Down, 0.0, 0.0, 0), Some(&st)).unwrap();
    let mut mouse_up = pe(PointerEventKind::Up, 1.0, 1.0, 8_000);
    mouse_up.is_mouse_synthesized = true;
    assert_eq!(engine.handle_pointer(mouse_up, None).unwrap(), InkEffect::Ignored);
    assert_eq!(engine.active_state(), anim_editor::StrokeState::Capturing);

    // Wrong pointer id cannot inject or terminate.
    let mut other = pe(PointerEventKind::Update, 2.0, 2.0, 9_000);
    other.pointer_id = 99;
    assert_eq!(engine.handle_pointer(other, None).unwrap(), InkEffect::Ignored);
}

#[test]
fn capture_loss_cancels_without_commit() {
    let mut engine = InkEngine::new(Tool::Pen);
    let st = setup(1.0, (0.0, 0.0));
    engine.handle_pointer(pe(PointerEventKind::Down, 0.0, 0.0, 0), Some(&st)).unwrap();
    engine.handle_pointer(pe(PointerEventKind::Update, 1.0, 1.0, 8_000), None).unwrap();

    let effect = engine
        .handle_pointer(pe(PointerEventKind::CaptureLost, 1.0, 1.0, 9_000), None)
        .unwrap();
    assert_eq!(effect, InkEffect::Cancelled);
    assert_eq!(engine.active_state(), anim_editor::StrokeState::Idle);
    assert!(engine.pending_commit().is_none());

    // Nothing to commit afterwards.
    assert!(engine.pending_commit().is_none());
    // Committing a fresh offset returns no spec; session-level Ok(None).
}

#[test]
fn eraser_routes_to_explicit_command_not_draw() {
    let mut engine = InkEngine::new(Tool::Eraser);
    let st = setup(1.0, (0.0, 0.0));
    let effect =
        engine.handle_pointer(pe(PointerEventKind::Down, 0.0, 0.0, 0), Some(&st)).unwrap();
    assert_eq!(effect, InkEffect::EraserCommandIssued);
    assert_eq!(engine.active_state(), anim_editor::StrokeState::Idle);
}

#[test]
fn malformed_events_rejected_without_state_change() {
    let mut engine = InkEngine::new(Tool::Pen);
    let st = setup(1.0, (0.0, 0.0));
    engine.handle_pointer(pe(PointerEventKind::Down, 0.0, 0.0, 0), Some(&st)).unwrap();

    // Nonfinite down would have started; here: malformed UPDATE rejected.
    let bad = pe(PointerEventKind::Update, f32::NAN, 0.0, 8_000);
    assert!(matches!(
        engine.handle_pointer(bad, None),
        Err(anim_editor::EditorError::InvalidPointerData(_))
    ));
    // State preserved: a valid update still appends.
    engine.handle_pointer(pe(PointerEventKind::Update, 1.0, 1.0, 9_000), None).unwrap();
    assert_eq!(engine.active_stroke().unwrap().points.len(), 2);

    // Nonfinite pressure rejected.
    let bad_p = pe(PointerEventKind::Update, 1.0, 1.0, 10_000);
    let mut bad_p = bad_p;
    bad_p.pressure = f32::NAN;
    assert!(matches!(
        engine.handle_pointer(bad_p, None),
        Err(anim_editor::EditorError::InvalidPointerData(_))
    ));

    // History not ordered oldest->newest rejected wholesale.
    let bad_hist = PointerEvent {
        kind: PointerEventKind::Update,
        pointer_id: 7,
        is_primary: true,
        is_eraser: false,
        is_mouse_synthesized: false,
        client_x: 2.0,
        client_y: 2.0,
        pressure: 0.5,
        tilt_x: 0,
        tilt_y: 0,
        timestamp_us: 20_000,
        history: vec![
            RawPoint {
                client_x: 1.0,
                client_y: 1.0,
                pressure: 0.5,
                tilt_x: 0,
                tilt_y: 0,
                timestamp_us: 30_000,
            },
            RawPoint {
                client_x: 1.5,
                client_y: 1.5,
                pressure: 0.5,
                tilt_x: 0,
                tilt_y: 0,
                timestamp_us: 29_000,
            },
        ],
    };
    assert!(matches!(
        engine.handle_pointer(bad_hist, None),
        Err(anim_editor::EditorError::InvalidPointerData(_))
    ));
}

#[test]
fn pressure_normalized_and_clamped() {
    let mut engine = InkEngine::new(Tool::Pen);
    let st = setup(1.0, (0.0, 0.0));
    let mut ev = pe(PointerEventKind::Down, 0.0, 0.0, 0);
    ev.pressure = 1.7; // out of device range; engine clamps defensively
    engine.handle_pointer(ev, Some(&st)).unwrap();
    let p0 = engine.active_stroke().unwrap().points[0].pressure;
    assert_eq!(p0, 1.0);

    let mut ev2 = pe(PointerEventKind::Update, 1.0, 1.0, 8_000);
    ev2.pressure = -0.25;
    engine.handle_pointer(ev2, None).unwrap();
    let p1 = engine.active_stroke().unwrap().points[1].pressure;
    assert_eq!(p1, 0.0);
}

#[test]
fn excessive_points_cancel_with_diagnostic_never_commit() {
    let mut engine = InkEngine::new(Tool::Pen);
    let st = setup(1.0, (0.0, 0.0));
    engine.handle_pointer(pe(PointerEventKind::Down, 0.0, 0.0, 0), Some(&st)).unwrap();

    let limit = anim_editor::MAX_STROKE_POINTS;
    // Down contributes point 1; limit-1 updates fill the point budget.
    for i in 1..limit {
        let r = engine.handle_pointer(
            pe(PointerEventKind::Update, i as f32, i as f32, i as u64 * 1_000),
            None,
        );
        assert!(r.is_ok(), "i={i}");
    }
    assert_eq!(engine.active_stroke().unwrap().points.len(), limit as usize);
    // One more exceeds the configured maximum -> typed cancel diagnostic.
    let over = engine.handle_pointer(
        pe(PointerEventKind::Update, (limit + 1) as f32, 0.0, (limit as u64 + 1) * 1_000),
        None,
    );
    match over {
        Err(anim_editor::EditorError::MaxPointsExceeded { limit: l, received }) => {
            assert_eq!(l, limit);
            assert_eq!(received, limit + 1);
        }
        other => panic!("expected MaxPointsExceeded, got {other:?}"),
    }
    // Stroke cancelled; nothing pending to commit.
    assert_eq!(engine.active_state(), anim_editor::StrokeState::Idle);
    assert!(engine.pending_commit().is_none());
}

#[test]
fn transient_submission_failure_retains_same_command_id() {
    // Engine level: pending spec is stable across repeated reads (retries).
    let mut engine = InkEngine::new(Tool::Pen);
    let st = setup(1.0, (0.0, 0.0));
    engine.handle_pointer(pe(PointerEventKind::Down, 0.0, 0.0, 0), Some(&st)).unwrap();
    let up_effect =
        engine.handle_pointer(pe(PointerEventKind::Up, 2.0, 2.0, 8_000), None).unwrap();
    let first_id = match up_effect {
        InkEffect::StrokeUp { command_id, .. } => command_id,
        other => panic!("expected StrokeUp, got {other:?}"),
    };

    let pending_a = engine.pending_commit().unwrap().clone();
    let _pending_b = engine.pending_commit().unwrap();
    assert_eq!(pending_a.command_id, first_id, "same id on retry");
    assert_eq!(pending_a.points.len(), 2);

    // Rejected submission: the user explicitly cancels.
    engine.cancel_pending_after_rejection();
    assert!(engine.pending_commit().is_none());
    assert_eq!(engine.active_state(), anim_editor::StrokeState::Idle);
    assert_eq!(engine.last_committed_command_id(), None, "no commit leaked");
}

#[test]
fn session_commit_path_and_single_undo_entry() {
    let doc = ProjectDocument::new(FrameRate::new(24, 1).unwrap());
    let mut proc = CommandProcessor::new(doc);

    let scene_id = SceneId::new_v4();
    let node_id = NodeId::new_v4();
    {
        let doc = proc.document();
        let envelope = anim_commands::CommandEnvelope::new(
            doc,
            "setup",
            vec![
                SetupCommand::UpsertScene { scene_id, name: "s".into() },
                SetupCommand::UpsertNode {
                    scene_id,
                    node_id,
                    name: "ink".into(),
                    parent: None,
                    asset: None,
                },
            ],
        );
        proc.apply(envelope).unwrap();
    }

    let session = EditorSession::new(proc, 0, 16_666.0, Tool::Pen);
    session.set_tool(Tool::Pen);

    let st = StrokeSetup {
        view_scale: 1.0,
        view_offset_x: 0.0,
        view_offset_y: 0.0,
        scene_id,
        node_id,
        key_time_num: 12,
    };
    session.handle_pointer(pe(PointerEventKind::Down, 0.0, 0.0, 0), Some(st)).unwrap();
    session.handle_pointer(pe(PointerEventKind::Update, 1.0, 1.0, 8_000), None).unwrap();
    // Transient point is visible ONLY in the overlay, never the snapshot.
    let snap_before = session.current();
    assert_eq!(
        snap_before.undo_depth, 1,
        "setup transaction only; stroke not yet committed"
    );

    let up_effect =
        session.handle_pointer(pe(PointerEventKind::Up, 2.0, 2.0, 16_000), None).unwrap();
    assert!(matches!(up_effect, InkEffect::StrokeUp { .. }));

    let receipt = session.commit_stroke().unwrap().expect("one receipt");
    assert!(receipt.new_revision > snap_before.project_revision);

    // EXACTLY ONCE: a repeat commit returns None; undo depth stays 2
    // (setup + stroke). Duplicate events produced no extra commands.
    assert!(session.commit_stroke().unwrap().is_none());
    let snap_after = session.current();
    assert_eq!(snap_after.undo_depth, 2, "one undo entry for the stroke");
    assert_eq!(snap_after.last_command_id, Some(receipt.command_id));
}

#[test]
fn disabled_tool_passes_events_through() {
    let mut engine = InkEngine::new(Tool::Disabled);
    let st = setup(1.0, (0.0, 0.0));
    let effect =
        engine.handle_pointer(pe(PointerEventKind::Down, 0.0, 0.0, 0), Some(&st)).unwrap();
    // Ignored: egui keeps the events (mouse editing preserved, spec 19.11).
    assert_eq!(effect, InkEffect::Ignored);
}
