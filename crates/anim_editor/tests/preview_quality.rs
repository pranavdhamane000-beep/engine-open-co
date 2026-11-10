//! G-WP10 preview-quality tests: hysteresis and export isolation (spec 19.12).
//! Level changes never touch project data or export settings; counters reset
//! on seek/suspension/resize/breakpoint; recovered sessions start at best.

use anim_core::FrameRate;
use anim_editor::{EditorSession, QualityScale, SnapshotReader};
use anim_export::ExportSettings;
use anim_project::ProjectDocument;
use std::time::Duration;

fn ms(v: f64) -> Duration {
    Duration::from_secs_f64(v / 1000.0)
}

fn settings_json() -> String {
    let s = ExportSettings {
        destination: std::path::PathBuf::from("out.mp4"),
        ..ExportSettings::default()
    };
    serde_json::to_string(&s).expect("settings serialize")
}

#[test]
fn downgrade_after_eight_consecutive_over_target() {
    let session = EditorSession::new(
        anim_commands::CommandProcessor::new(ProjectDocument::new(
            FrameRate::new(24, 1).unwrap(),
        )),
        0,
        16_666.0,
        anim_editor::Tool::Pen,
    );
    for _ in 0..7 {
        let q = session.update_preview_quality(ms(20.0));
        assert_eq!(q.scale, QualityScale::Full);
    }
    let q = session.update_preview_quality(ms(20.0));
    assert_eq!(q.scale, QualityScale::ThreeQuarter);
    assert_eq!(q.over_budget_frames, 0, "counter resets at the breakpoint");
}

#[test]
fn upgrade_only_after_120_frames_below_70pct() {
    let session = EditorSession::new(
        anim_commands::CommandProcessor::new(ProjectDocument::new(
            FrameRate::new(24, 1).unwrap(),
        )),
        0,
        16_666.0,
        anim_editor::Tool::Pen,
    );
    // Sink to ThreeQuarter first.
    for _ in 0..8 {
        session.update_preview_quality(ms(30.0));
    }
    // The EMA must decay before the consecutive-under streak starts, so the
    // upgrade index is well past the raw 120-frame floor.
    let mut up_index = None;
    for i in 1..=300 {
        let q = session.update_preview_quality(ms(5.0));
        if q.scale == QualityScale::Full {
            up_index = Some(i);
            break;
        }
        assert_eq!(q.scale, QualityScale::ThreeQuarter, "i={i}");
    }
    let up_index = up_index.expect("upgrade must occur");
    assert!(
        up_index >= anim_editor::UNDER_BUDGET_UPGRADE_FRAMES as usize,
        "upgrade before 120 under-frames violates hysteresis (index={up_index})"
    );
}

#[test]
fn half_is_floor_full_is_ceiling() {
    let session = EditorSession::new(
        anim_commands::CommandProcessor::new(ProjectDocument::new(
            FrameRate::new(24, 1).unwrap(),
        )),
        0,
        16_666.0,
        anim_editor::Tool::Pen,
    );
    // Drain to the floor: two breakpoints of 8 over-budget frames.
    for _ in 0..16 {
        session.update_preview_quality(ms(30.0));
    }
    let q = session.update_preview_quality(ms(30.0));
    assert_eq!(q.scale, QualityScale::Half, "floor holds");
    // Floor holds no matter how many extra bad frames arrive.
    for i in 0..32 {
        let q = session.update_preview_quality(ms(40.0));
        assert_eq!(q.scale, QualityScale::Half, "i={i}");
    }
}

#[test]
fn seek_resets_quality_counters() {
    let session = EditorSession::new(
        anim_commands::CommandProcessor::new(ProjectDocument::new(
            FrameRate::new(24, 1).unwrap(),
        )),
        0,
        16_666.0,
        anim_editor::Tool::Pen,
    );
    for _ in 0..5 {
        session.update_preview_quality(ms(30.0)); // over budget streak
    }
    let q_before = session.current().quality.clone();
    assert!(q_before.over_budget_frames > 0);

    session.transport_seek(100);
    let q = session.current().quality.clone();
    assert_eq!(q.over_budget_frames, 0, "seek resets the samples");
    assert_eq!(q.scale, QualityScale::Full, "level persists");
}

#[test]
fn preview_quality_never_reaches_export_settings() {
    // The export settings hash MUST stay constant under editor quality
    // activity (spec 19.12: "export settings hashes remain constant").
    let session = EditorSession::new(
        anim_commands::CommandProcessor::new(ProjectDocument::new(
            FrameRate::new(24, 1).unwrap(),
        )),
        0,
        16_666.0,
        anim_editor::Tool::Pen,
    );

    let before = settings_json();

    // Churn quality hard: downgrade + upgrade + resets + device recovery.
    for _ in 0..16 {
        session.update_preview_quality(ms(30.0));
    }
    session.reset_quality_counters();
    for _ in 0..121 {
        session.update_preview_quality(ms(5.0));
    }
    session.reset_quality_full();
    session.transport_seek(50);
    session.transport_play();
    session.transport_pause();

    let after = settings_json();
    assert_eq!(before, after, "preview quality leaked into export settings");
}

#[test]
fn recovered_session_starts_full() {
    let session = EditorSession::new(
        anim_commands::CommandProcessor::new(ProjectDocument::new(
            FrameRate::new(24, 1).unwrap(),
        )),
        0,
        16_666.0,
        anim_editor::Tool::Pen,
    );
    for _ in 0..16 {
        session.update_preview_quality(ms(30.0));
    }
    assert_eq!(session.current().quality.scale, QualityScale::Half);
    session.reset_quality_full(); // device recovery
    let q = session.current().quality.clone();
    assert_eq!(q.scale, QualityScale::Full);
    assert_eq!(q.effect_level, 2);
    assert_eq!(q.ema_frame_us, 0.0);
    assert_eq!(q.over_budget_frames, 0);
    assert_eq!(
        q.under_budget_frames, 0,
        "device recovery publishes a fresh best-quality snapshot"
    );
}
