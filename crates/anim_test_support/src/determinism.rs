//! Fresh-process determinism runners and checkpoint-equivalence comparison
//! (spec §20.3 determinism.rs, §20.7 determinism gate algorithm).
//!
//! Contract: a frozen scene is rendered by TWO (or more) FRESH processes on
//! the pinned stack and every artifact hash must match exactly — CPU frame
//! inputs, events/simulation, and PCM (§20.6: PCM exact; image tolerance is
//! a golden-layer concern, never here). Cadence variation (checkpoint
//! cadence, audio chunk size, readback slots) MUST NOT change any hash.
//! The parent never trusts wall-clock; the child measures its own work and
//! reports peak RSS. First mismatch retains BOTH complete artifacts
//! (§20.11) before returning failure.

use crate::benchmark::Environment;
use crate::scenes::TestSceneId;
use anim_assets::hash::ContentHash;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Typed test-support error (spec §20.11: typed, nonzero status, artifacts
/// preserved by the caller before acting on the error).
#[derive(Debug, thiserror::Error)]
pub enum TestError {
    #[error("spawning or awaiting the runner process failed: {0}")]
    Process(String),
    #[error("runner wrote no parseable result at {0}: {1}")]
    RunOutput(PathBuf, String),
    #[error("invalid runner input: {0}")]
    InvalidInput(String),
    #[error("serialization failed: {0}")]
    Serialization(String),
    #[error("fault injection is disabled (test-faults feature OFF)")]
    FaultsDisabled,
    #[error("benchmark path is not wired yet: {0}")]
    NotImplemented(&'static str),
    #[error("{0}")]
    Mismatch(Mismatch),
}

impl From<Mismatch> for TestError {
    fn from(m: Mismatch) -> Self {
        TestError::Mismatch(m)
    }
}

/// Exact DeterminismReport shape (spec §20.8 data structures).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeterminismReport {
    pub frames: u64,
    pub frame_input_hash: ContentHash,
    pub pcm_hash: ContentHash,
    pub simulation_hash: ContentHash,
    pub event_hash: ContentHash,
    pub peak_rss_bytes: u64,
}

/// Tunables the determinism gate varies explicitly (spec §20.7: repeat with
/// different checkpoint cadence, audio chunk size and readback slot count).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunConfig {
    /// Checkpoint cadence in frames (varied, must not change hashes).
    pub checkpoint_cadence_frames: u64,
    /// Offline audio mix chunk size in samples (varied).
    pub audio_chunk_size_samples: u64,
    /// Render readback ring slot count (varied).
    pub readback_slot_count: u32,
    /// Absolute path of the headless runner binary to spawn fresh
    /// (`anim_headless`; a test passes `env!("CARGO_BIN_EXE_anim_headless")`).
    pub runner_exe: PathBuf,
    /// Directory this run writes `report.json` + retained artifacts to
    /// (fresh per run; retained on failure per §20.11).
    pub artifacts_dir: PathBuf,
}

impl RunConfig {
    /// The §20.7 controlled variant of a config: same scene, same hashes
    /// expected, different cadence/chunk/slots.
    pub fn equivalence_variant(&self) -> RunConfig {
        RunConfig {
            checkpoint_cadence_frames: self.checkpoint_cadence_frames * 2 + 7,
            audio_chunk_size_samples: (self.audio_chunk_size_samples / 2).max(64),
            readback_slot_count: (self.readback_slot_count % 3) + 2,
            runner_exe: self.runner_exe.clone(),
            artifacts_dir: self.artifacts_dir.join("variant_cadence"),
        }
    }
}

/// Complete artifacts of ONE fresh run: the report plus everything the first
/// mismatch must retain (environment metadata and configuration echo).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunArtifacts {
    pub report: DeterminismReport,
    pub config: RunConfig,
    pub environment: Environment,
}

/// Describes the FIRST mismatched field between two runs (spec §20.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mismatch {
    pub field: &'static str,
    pub a: String,
    pub b: String,
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "determinism mismatch in {}: a={} b={}", self.field, self.a, self.b)
    }
}

/// Compare two fresh runs field-by-field, EXACT everywhere (spec §20.6).
/// Returns `Ok(())` when every artifact hash, frame count and peak RSS are
/// identical; otherwise `Err(Mismatch)` for the first differing field.
pub fn compare_runs(a: &RunArtifacts, b: &RunArtifacts) -> Result<(), Mismatch> {
    let (ra, rb) = (&a.report, &b.report);
    let checks: [(&'static str, String, String); 7] = [
        ("frames", ra.frames.to_string(), rb.frames.to_string()),
        (
            "frame_input_hash",
            ra.frame_input_hash.full_hex(),
            rb.frame_input_hash.full_hex(),
        ),
        ("pcm_hash", ra.pcm_hash.full_hex(), rb.pcm_hash.full_hex()),
        ("simulation_hash", ra.simulation_hash.full_hex(), rb.simulation_hash.full_hex()),
        ("event_hash", ra.event_hash.full_hex(), rb.event_hash.full_hex()),
        ("peak_rss_bytes", ra.peak_rss_bytes.to_string(), rb.peak_rss_bytes.to_string()),
        (
            "environment",
            serde_json::to_string(&a.environment).unwrap_or_else(|e| e.to_string()),
            serde_json::to_string(&b.environment).unwrap_or_else(|e| e.to_string()),
        ),
    ];
    for (field, va, vb) in checks {
        if va != vb {
            return Err(Mismatch { field, a: va, b: vb });
        }
    }
    Ok(())
}

/// Serialize and load one run's artifacts at `dir/report.json`.
fn write_artifacts(dir: &Path, artifacts: &RunArtifacts) -> Result<(), TestError> {
    std::fs::create_dir_all(dir).map_err(|e| TestError::Process(e.to_string()))?;
    let bytes = serde_json::to_vec_pretty(artifacts)
        .map_err(|e| TestError::Serialization(e.to_string()))?;
    std::fs::write(dir.join("report.json"), bytes)
        .map_err(|e| TestError::Process(e.to_string()))
}

fn read_artifacts(dir: &Path) -> Result<RunArtifacts, TestError> {
    let path = dir.join("report.json");
    let bytes = std::fs::read(&path)
        .map_err(|e| TestError::RunOutput(path.clone(), e.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|e| TestError::RunOutput(path, e.to_string()))
}

/// Run ONE fresh headless process for `frames` frames over the frozen scene
/// at `scene` with `config` (spec §20.7). The runner CLI surface (chunk 2)
/// is `anim_headless determinism --scene <p> --frames <n> --cadence <f>
/// --audio-chunk <s> --readback-slots <k> --out <dir>`; the child measures
/// its own peak RSS and writes `report.json` into `--out`.
fn run_one(
    scene: &Path,
    frames: u64,
    config: &RunConfig,
) -> Result<RunArtifacts, TestError> {
    write_artifacts(
        &config.artifacts_dir,
        &RunArtifacts {
            report: DeterminismReport {
                frames: 0,
                frame_input_hash: ContentHash([0u8; 32]),
                pcm_hash: ContentHash([0u8; 32]),
                simulation_hash: ContentHash([0u8; 32]),
                event_hash: ContentHash([0u8; 32]),
                peak_rss_bytes: 0,
            },
            config: config.clone(),
            environment: Environment::capture(None),
        },
    )?;

    let out_dir = config.artifacts_dir.clone();
    let status = Command::new(&config.runner_exe)
        .arg("determinism")
        .arg("--scene")
        .arg(scene)
        .arg("--frames")
        .arg(frames.to_string())
        .arg("--cadence")
        .arg(config.checkpoint_cadence_frames.to_string())
        .arg("--audio-chunk")
        .arg(config.audio_chunk_size_samples.to_string())
        .arg("--readback-slots")
        .arg(config.readback_slot_count.to_string())
        .arg("--out")
        .arg(&out_dir)
        .status()
        .map_err(|e| TestError::Process(format!("{}: {e}", config.runner_exe.display())))?;

    if !status.success() {
        return Err(TestError::Process(format!(
            "runner exited {status:?} (failures retain artifacts per §20.11)"
        )));
    }
    read_artifacts(&config.artifacts_dir)
}

/// The §20.7 determinism gate: two fresh processes over the SAME frozen
/// scene with the SAME config must agree exactly; then the equivalence
/// variant (different cadence/chunk/slots) must ALSO agree. Returns the
/// primary report. Timeouts/crashes propagate as typed failures and every
/// artifact stays on disk for review.
pub fn run_determinism(
    scene: &Path,
    frames: u64,
    config: &RunConfig,
) -> Result<DeterminismReport, TestError> {
    if frames == 0 {
        return Err(TestError::InvalidInput("frames must be nonzero".into()));
    }
    if scene.extension().is_none() {
        return Err(TestError::InvalidInput("scene path must name a project file".into()));
    }

    // Primary pair: two INDEPENDENT fresh processes.
    let mut dir_a = config.artifacts_dir.clone();
    dir_a.push("run_a");
    let mut dir_b = config.artifacts_dir.clone();
    dir_b.push("run_b");
    let cfg_a = RunConfig { artifacts_dir: dir_a, ..config.clone() };
    let cfg_b = RunConfig { artifacts_dir: dir_b, ..config.clone() };

    let a = run_one(scene, frames, &cfg_a)?;
    let b = run_one(scene, frames, &cfg_b)?;
    compare_runs(&a, &b)?;

    // Cadence-equivalence: different cadence/chunk/slots, same hashes.
    let variant_cfg = config.equivalence_variant();
    let v = run_one(scene, frames, &variant_cfg)?;
    compare_runs(&a, &v)?;

    Ok(a.report)
}

/// Convenience for chunk-2 integration tests: run the gate over a
/// synthesized frozen scene, written to a tempdir first (spec §20.4: no
/// unversioned external assets; scenes come from `TestSceneId`).
pub fn frozen_scene_path(
    temp_root: &Path,
    scene_id: TestSceneId,
) -> Result<PathBuf, TestError> {
    let bundle = crate::scenes::build(scene_id);
    let dir = temp_root.join(scene_id.dir_name());
    std::fs::create_dir_all(&dir).map_err(|e| TestError::Process(e.to_string()))?;
    // Canonical scene text the headless runner replays deterministically:
    // project id, scene id, manifest — all fixed UUID constants, so the file
    // is byte-stable across processes.
    let text = format!(
        "{}\n{}\n{}",
        bundle.document.project_id,
        bundle.scene_id,
        crate::scenes::manifest_json(&bundle)
    );
    let path = dir.join("scene.frozen");
    std::fs::write(&path, text).map_err(|e| TestError::Process(e.to_string()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(frames: u64, pcm: [u8; 32]) -> DeterminismReport {
        DeterminismReport {
            frames,
            frame_input_hash: ContentHash([1u8; 32]),
            pcm_hash: ContentHash(pcm),
            simulation_hash: ContentHash([3u8; 32]),
            event_hash: ContentHash([4u8; 32]),
            peak_rss_bytes: 1_000_000,
        }
    }

    fn artifacts(frames: u64, pcm: [u8; 32]) -> RunArtifacts {
        RunArtifacts {
            report: report(frames, pcm),
            config: RunConfig {
                checkpoint_cadence_frames: 120,
                audio_chunk_size_samples: 1024,
                readback_slot_count: 4,
                runner_exe: PathBuf::from("runner.exe"),
                artifacts_dir: PathBuf::from("artifacts"),
            },
            environment: Environment::default(),
        }
    }

    #[test]
    fn identical_runs_compare_clean() {
        assert!(compare_runs(&artifacts(96, [9u8; 32]), &artifacts(96, [9u8; 32])).is_ok());
    }

    #[test]
    fn first_mismatch_is_named_with_both_values() {
        let err = compare_runs(&artifacts(96, [9u8; 32]), &artifacts(96, [1u8; 32]))
            .expect_err("pcm differs");
        assert_eq!(err.field, "pcm_hash");
        assert_ne!(err.a, err.b);
    }

    #[test]
    fn equivalence_variant_changes_only_tunables() {
        let cfg = RunConfig {
            checkpoint_cadence_frames: 100,
            audio_chunk_size_samples: 1024,
            readback_slot_count: 4,
            runner_exe: PathBuf::from("r.exe"),
            artifacts_dir: PathBuf::from("d"),
        };
        let v = cfg.equivalence_variant();
        assert_ne!(v.checkpoint_cadence_frames, cfg.checkpoint_cadence_frames);
        assert_ne!(v.audio_chunk_size_samples, cfg.audio_chunk_size_samples);
        assert_ne!(v.readback_slot_count, cfg.readback_slot_count);
        assert_eq!(v.runner_exe, cfg.runner_exe);
    }

    #[test]
    fn zero_frames_is_typed_invalid_input() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = RunConfig {
            checkpoint_cadence_frames: 1,
            audio_chunk_size_samples: 1,
            readback_slot_count: 1,
            runner_exe: PathBuf::from("definitely-not-a-real-binary"),
            artifacts_dir: dir.path().to_path_buf(),
        };
        let scene_path = dir.path().join("scene.frozen");
        std::fs::write(&scene_path, b"x").expect("write");
        let err = run_determinism(&scene_path, 0, &cfg).expect_err("zero frames");
        assert!(matches!(err, TestError::InvalidInput(_)));
    }
}
