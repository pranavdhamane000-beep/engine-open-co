//! Fresh-process determinism gate via CARGO_BIN_EXE_anim_headless
//! (spec §20.7: two independent processes over the same frozen scene must
//! agree exactly on all artifact hashes and frame counts).
//!
//! This test spawns `anim_headless determinism` twice as fresh processes
//! (matching the §20.7 "two fresh processes" requirement) and compares
//! the resulting reports field-by-field.

use anim_assets::hash::ContentHash;
use std::process::Command;

/// Determinism report matching the CLI output schema.
#[derive(serde::Deserialize, Debug, PartialEq)]
struct DeterminismReport {
    frames: u64,
    frame_input_hash: String,
    pcm_hash: String,
    simulation_hash: String,
    event_hash: String,
    peak_rss_bytes: u64,
}

#[test]
fn determinism_two_fresh_processes_agree() {
    // Build a frozen scene file inline (project_id, scene_id, manifest JSON).
    let project_id = "1e5f0000-0000-4000-8000-000000000001";
    let scene_id = "2a5f0000-0000-4000-8000-00000000000a";
    let manifest = r#"{"scene_id":"TestA","title":"Test A — Full body","fps_num":24,"fps_den":1,"width":1280,"height":720,"duration_frames":96,"node_count":4,"track_count":4,"event_count":2,"resource_counts":{"meshes":0,"bones":4,"particles":0,"lights":0},"image_tolerance":2,"pcm_tolerance":0}"#;
    let scene_text = format!("{project_id}\n{scene_id}\n{manifest}");

    // Write to a temp file.
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let scene_path = temp_dir.path().join("scene.frozen");
    std::fs::write(&scene_path, scene_text).expect("write frozen scene");

    // Output directories for the two runs.
    let out_a = temp_dir.path().join("run_a");
    let out_b = temp_dir.path().join("run_b");
    std::fs::create_dir_all(&out_a).expect("mkdir run_a");
    std::fs::create_dir_all(&out_b).expect("mkdir run_b");

    // Runner binary path (set by cargo during test compilation).
    let runner = std::env::var("CARGO_BIN_EXE_anim_headless")
        .expect("CARGO_BIN_EXE_anim_headless not set; test must run via cargo test");

    // Run A
    let status_a = Command::new(&runner)
        .arg("determinism")
        .arg("--scene")
        .arg(&scene_path)
        .arg("--frames")
        .arg("96")
        .arg("--cadence")
        .arg("120")
        .arg("--audio-chunk")
        .arg("1024")
        .arg("--readback-slots")
        .arg("4")
        .arg("--out")
        .arg(&out_a)
        .status()
        .expect("spawn run_a");
    assert!(status_a.success(), "run_a failed: {status_a}");

    // Run B (fresh process)
    let status_b = Command::new(&runner)
        .arg("determinism")
        .arg("--scene")
        .arg(&scene_path)
        .arg("--frames")
        .arg("96")
        .arg("--cadence")
        .arg("120")
        .arg("--audio-chunk")
        .arg("1024")
        .arg("--readback-slots")
        .arg("4")
        .arg("--out")
        .arg(&out_b)
        .status()
        .expect("spawn run_b");
    assert!(status_b.success(), "run_b failed: {status_b}");

    // Load both reports.
    let report_a = load_report(&out_a.join("report.json"));
    let report_b = load_report(&out_b.join("report.json"));

    // Compare all fields EXACTLY (spec §20.6: PCM exact; image tolerance is golden-layer only).
    assert_eq!(report_a.frames, report_b.frames, "frame count differs");
    assert_eq!(
        report_a.frame_input_hash, report_b.frame_input_hash,
        "frame_input_hash differs"
    );
    assert_eq!(
        report_a.pcm_hash, report_b.pcm_hash,
        "pcm_hash differs (must be exact per §20.6)"
    );
    assert_eq!(
        report_a.simulation_hash, report_b.simulation_hash,
        "simulation_hash differs"
    );
    assert_eq!(report_a.event_hash, report_b.event_hash, "event_hash differs");
    // peak_rss_bytes may differ slightly due to measurement noise; assert same order of magnitude.
    let rss_ratio = (report_a.peak_rss_bytes.max(report_b.peak_rss_bytes) as f64)
        / (report_a.peak_rss_bytes.min(report_b.peak_rss_bytes) as f64);
    assert!(
        rss_ratio < 1.5,
        "peak RSS differs by >50%: a={} b={}",
        report_a.peak_rss_bytes,
        report_b.peak_rss_bytes
    );

    // Sanity: hashes should be valid 64-char hex.
    for hash in [
        &report_a.frame_input_hash,
        &report_a.pcm_hash,
        &report_a.simulation_hash,
        &report_a.event_hash,
    ] {
        assert_eq!(hash.len(), 64, "hash must be 64 hex chars: {hash}");
        ContentHash::parse(hash).expect("valid blake3 hex");
    }
}

/// Cadence-equivalence variant (§20.7): different cadence/chunk/slots must
/// produce IDENTICAL hashes.
#[test]
fn determinism_cadence_variant_produces_identical_hashes() {
    let project_id = "1e5f0000-0000-4000-8000-000000000001";
    let scene_id = "2a5f0000-0000-4000-8000-00000000000a";
    let manifest = r#"{"scene_id":"TestA","title":"Test A — Full body","fps_num":24,"fps_den":1,"width":1280,"height":720,"duration_frames":96,"node_count":4,"track_count":4,"event_count":2,"resource_counts":{"meshes":0,"bones":4,"particles":0,"lights":0},"image_tolerance":2,"pcm_tolerance":0}"#;
    let scene_text = format!("{project_id}\n{scene_id}\n{manifest}");

    let temp_dir = tempfile::tempdir().expect("tempdir");
    let scene_path = temp_dir.path().join("scene.frozen");
    std::fs::write(&scene_path, scene_text).expect("write frozen scene");

    let runner = std::env::var("CARGO_BIN_EXE_anim_headless")
        .expect("CARGO_BIN_EXE_anim_headless not set");

    // Primary config (baseline)
    let out_base = temp_dir.path().join("base");
    std::fs::create_dir_all(&out_base).expect("mkdir base");
    run_determinism(&runner, &scene_path, &out_base, 96, 120, 1024, 4);

    // Variant config: different cadence, audio chunk, readback slots
    let out_var = temp_dir.path().join("variant");
    std::fs::create_dir_all(&out_var).expect("mkdir variant");
    run_determinism(&runner, &scene_path, &out_var, 96, 247, 576, 5);

    let base = load_report(&out_base.join("report.json"));
    let variant = load_report(&out_var.join("report.json"));

    // All hashes MUST match exactly (spec §20.7: "different checkpoint cadence,
    // audio chunk size and readback slot count... MUST NOT change any hash")
    assert_eq!(base.frame_input_hash, variant.frame_input_hash);
    assert_eq!(base.pcm_hash, variant.pcm_hash);
    assert_eq!(base.simulation_hash, variant.simulation_hash);
    assert_eq!(base.event_hash, variant.event_hash);
}

fn run_determinism(
    runner: &str,
    scene_path: &std::path::Path,
    out_dir: &std::path::Path,
    frames: u64,
    cadence: u64,
    audio_chunk: u64,
    readback_slots: u32,
) {
    let status = Command::new(runner)
        .arg("determinism")
        .arg("--scene")
        .arg(scene_path)
        .arg("--frames")
        .arg(frames.to_string())
        .arg("--cadence")
        .arg(cadence.to_string())
        .arg("--audio-chunk")
        .arg(audio_chunk.to_string())
        .arg("--readback-slots")
        .arg(readback_slots.to_string())
        .arg("--out")
        .arg(out_dir)
        .status()
        .expect("spawn determinism runner");
    assert!(status.success(), "determinism run failed: {status}");
}

fn load_report(path: &std::path::Path) -> DeterminismReport {
    let text = std::fs::read_to_string(path).expect("read report.json");
    serde_json::from_str(&text).expect("parse report.json")
}
