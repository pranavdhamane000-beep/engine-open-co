//! Benchmark execution, timing/memory records and regression policy
//! (spec §20.3 benchmark.rs, §20.7 benchmark algorithm, §20.11).
//!
//! For each Test A–E: THREE warm-up runs, then AT LEAST ten measured runs.
//! Record median / p95 / p99 frame time, export throughput, peak RSS,
//! observed/estimated GPU memory, readback wait and encode time — keyed by
//! a machine-readable ENVIRONMENT hash every record carries (§20.6: toolchain,
//! engine version, adapter/driver, feature flags).
//!
//! Regression policy (§20.7, §20.11): compare ONLY against matched-machine
//! (same environment-hash) baselines. An environment change creates a new
//! baseline; thresholds NEVER relax automatically and regressions never
//! rewrite baselines.
//!
//! Chunk-1 honesty note: the frame harness below measures the
//! deterministic scene-bundle pipeline through production APIs
//! (anim_project document/manifest construction). Rendering/acceptance-coupled
//! frame timing lands with chunk 3 (spec §20.5 step 8); the record type,
//! percentile math and environment hashing are final NOW so later chunks
//! only swap the measured closure.

use crate::determinism::TestError;
use crate::scenes::{self, TestSceneId};
use anim_assets::hash::ContentHash;
use serde::{Deserialize, Serialize};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Exact BenchmarkResult shape (spec §20.8 data structures).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkResult {
    pub scene: String,
    pub median_frame_us: u64,
    pub p95_frame_us: u64,
    pub p99_frame_us: u64,
    pub peak_rss_bytes: u64,
    pub export_frames_per_second: f64,
    pub environment_hash: ContentHash,
}

/// Machine-readable environment metadata (§20.6: "every run records ...
/// Rust toolchain, engine commit, FFmpeg version/hash, adapter/driver and
/// feature flags"). The hash of this record is the benchmark key.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Environment {
    pub engine_version: String,
    pub rust_toolchain: String,
    pub engine_commit: String,
    pub ffmpeg_version_hash: String,
    pub adapter_driver: String,
    pub feature_flags: Vec<String>,
    pub recorded_at: Option<String>,
}

impl Environment {
    /// Capture what is verifiable OFFLINE (no network, §20.4). Adapter and
    /// FFmpeg identities are supplied by the caller when the machine state
    /// is known (wp00 probe output); everything else is from the build.
    pub fn capture(adapter: Option<String>) -> Self {
        Environment {
            engine_version: env!("CARGO_PKG_VERSION").to_string(),
            rust_toolchain: option_env!("RUSTC_VERSION")
                .unwrap_or("rustc-unrecorded")
                .to_string(),
            engine_commit: option_env!("ENGINE_COMMIT")
                .unwrap_or("uncommitted")
                .to_string(),
            ffmpeg_version_hash: String::new(),
            adapter_driver: adapter.unwrap_or_else(|| "adapter-unrecorded".into()),
            // Only declared feature flags of THIS crate are recorded.
            feature_flags: if cfg!(feature = "test-faults") {
                vec!["test-faults".into()]
            } else {
                Vec::new()
            },
            recorded_at: Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    .to_string(),
            ),
        }
    }

    /// Canonical content hash of the environment record. `recorded_at` is
    /// EXCLUDED — two records of the same machine state must collide, or
    /// every run would mint a new baseline (§20.7).
    pub fn hash(&self) -> ContentHash {
        let key = Environment { recorded_at: None, ..self.clone() };
        let bytes = serde_json::to_vec(&key).expect("env serialize");
        scenes::hash_bytes(&bytes)
    }
}

/// Nearest-rank percentile over UNSORTED integral samples (us). Nearest
/// rank: p95 of 10 samples = 10th smallest (index ceil(9.5)-1). Median of
/// even counts averages the two middle samples.
pub fn percentiles(samples: &[u64]) -> (u64, u64, u64) {
    assert!(!samples.is_empty(), "percentiles need samples");
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let n = sorted.len();
    let median = if n % 2 == 1 {
        sorted[n / 2]
    } else {
        // Split integer median: floor + ceil then average with rounding
        // (integer-only, deterministic).
        let a = sorted[n / 2 - 1];
        let b = sorted[n / 2];
        a + (b - a).div_ceil(2)
    };
    let pick = |pct: f64| -> u64 {
        let rank = ((pct / 100.0) * n as f64).ceil().max(1.0) as usize;
        sorted[(rank - 1).min(n - 1)]
    };
    (median, pick(95.0), pick(99.0))
}

/// Peak RSS of THIS process via the Windows performance API. On non-Windows
/// callers get a typed error (this engine targets Windows; §9 platform).
pub fn peak_rss_bytes() -> Result<u64, TestError> {
    #[cfg(windows)]
    {
        use windows::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
        };
        let mut counters = PROCESS_MEMORY_COUNTERS_EX {
            cb: u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>())
                .expect("counter size"),
            PageFaultCount: 0,
            PeakWorkingSetSize: 0,
            WorkingSetSize: 0,
            QuotaPeakPagedPoolUsage: 0,
            QuotaPagedPoolUsage: 0,
            QuotaPeakNonPagedPoolUsage: 0,
            QuotaNonPagedPoolUsage: 0,
            PagefileUsage: 0,
            PeakPagefileUsage: 0,
            PrivateUsage: 0,
        };
        let self_handle = unsafe { windows::Win32::System::Threading::GetCurrentProcess() };
        let ok = unsafe {
            GetProcessMemoryInfo(
                self_handle,
                &mut counters as *mut PROCESS_MEMORY_COUNTERS_EX
                    as *mut PROCESS_MEMORY_COUNTERS,
                counters.cb,
            )
        };
        if ok.is_err() {
            return Err(TestError::Process("GetProcessMemoryInfo failed".into()));
        }
        Ok(u64::try_from(counters.PeakWorkingSetSize).unwrap_or(0))
    }
    #[cfg(not(windows))]
    {
        Err(TestError::Process("peak_rss_bytes: no non-Windows RSS source".into()))
    }
}

/// A scene benchmark harness: the measured unit is ONE frame-equivalent of
/// work per call. `runner` is production code under test; warm-ups are run
/// 3 times and discarded (§20.7), then `runs` measured samples are taken.
pub struct BenchmarkHarness {
    /// Scene under test; recorded for harness identity and chunk-3 use.
    pub scene: TestSceneId,
    /// 3 warm-ups + 10 measured is the minimum spec-compliant run.
    warmups: usize,
    runs: usize,
}

impl BenchmarkHarness {
    pub fn new(scene: TestSceneId) -> Self {
        Self { scene, warmups: 3, runs: 10 }
    }

    /// Assert spec minimums are adjustable only UPWARD (never below).
    pub fn with_counts(mut self, warmups: usize, runs: usize) -> Self {
        self.warmups = self.warmups.max(warmups).max(3);
        self.runs = self.runs.max(runs).max(10);
        self
    }

    /// Execute warm-ups + measured runs of `runner` (each call = one
    /// frame-equivalent chunk), collect median/p95/p99 (§20.7).
    pub fn measure(&self, mut runner: impl FnMut() -> u64) -> (u64, u64, u64) {
        for _ in 0..self.warmups {
            let _ = runner();
        }
        let mut samples = Vec::with_capacity(self.runs);
        for _ in 0..self.runs {
            samples.push(runner());
        }
        percentiles(&samples)
    }
}

/// Exact §20.9 interface: `benchmark_test(scene_id) -> Result<BenchmarkResult, TestError>`.
///
/// Chunk-1 runner: measures the DETERMINISTIC PART of the acceptance scene
/// (bundle = document + manifest construction through production APIs).
/// Emits throughput for the same work and real peak RSS. Chunk 3 replaces
/// the measured closure with render/acceptance timing without changing
/// this function's signature or record semantics.
pub fn benchmark_test(scene_id: TestSceneId) -> Result<BenchmarkResult, TestError> {
    // Sanity: the frozen scene must build deterministically before timing.
    let bundle = scenes::build(scene_id);
    let _ = scenes::manifest_json(&bundle);

    let harness = BenchmarkHarness::new(scene_id);
    let (median, p95, p99) = harness.measure(|| {
        let start = Instant::now();
        // One frame-equivalent chunk: rebuild the whole frozen bundle
        // (all-FIXED ids; pure) + serialize its manifest.
        let b = scenes::build(scene_id);
        let json = scenes::manifest_json(&b);
        let _ = scenes::bundle_hash(&b);
        debug_assert!(!json.is_empty());
        start.elapsed().as_micros() as u64
    });

    let rss = peak_rss_bytes()?;

    // Throughput for THIS runner: frames of the scene per measured second.
    // Reported in export units (frames/s); the runner's cost is frame-time
    // median, so fps = frames / (median_us * runs / 1e6) conservatively.
    let frames_per_run = bundle.manifest.duration_frames as f64;
    let export_fps = if median > 0 {
        frames_per_run / (median as f64 * harness.runs as f64 / 1_000_000.0)
    } else {
        0.0
    };

    Ok(BenchmarkResult {
        scene: scene_id.dir_name().to_string(),
        median_frame_us: median,
        p95_frame_us: p95,
        p99_frame_us: p99,
        peak_rss_bytes: rss,
        export_frames_per_second: export_fps,
        environment_hash: Environment::capture(None).hash(),
    })
}

/// A recorded baseline this machine previously approved. Regression checks
/// NEVER write it (§20.11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkBaseline {
    pub result: BenchmarkResult,
    pub environment: Environment,
    pub approved_by: String,
    pub approved_at: String,
}

/// Regression verdict (§20.7: compare only matched-machine baselines).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Regression {
    /// Same environment hash; current slower than baseline by ratio.
    MatchedAndSlower(f64),
    /// Environment changed → a NEW baseline is created; never compared.
    EnvironmentChanged,
}

impl std::fmt::Display for Regression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Regression::MatchedAndSlower(r) => write!(f, "regressed {r:.3}× slower"),
            Regression::EnvironmentChanged => {
                write!(f, "environment changed — new baseline")
            }
        }
    }
}

/// Mandatory sliding-ratio policy (spec §20.7 "thresholds never relax
/// automatically"): any slowdown beyond `max_regression_ratio` against a
/// MATCHED baseline is a regression. Comparing an unmatched environment is
/// EnvironmentChanged — never silently passes as "regressed/ok".
pub fn check_regression(
    baseline: &BenchmarkBaseline,
    current: &BenchmarkResult,
    max_regression_ratio: f64,
) -> Result<Option<Regression>, TestError> {
    if !max_regression_ratio.is_finite() || max_regression_ratio <= 1.0 {
        return Err(TestError::InvalidInput(
            "max_regression_ratio must be finite and > 1.0".into(),
        ));
    }
    if current.environment_hash != baseline.result.environment_hash {
        // New environment → new baseline; recorded, not compared.
        return Ok(Some(Regression::EnvironmentChanged));
    }
    if baseline.result.median_frame_us == 0 {
        return Ok(None);
    }
    let ratio = current.median_frame_us as f64 / baseline.result.median_frame_us as f64;
    if ratio > max_regression_ratio {
        Ok(Some(Regression::MatchedAndSlower(ratio)))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_are_nearest_rank_and_median_is_split() {
        // 10 sorted samples: median = floor/ceil average of 5th and 6th.
        let s: Vec<u64> = [10, 20, 30, 40, 50, 60, 70, 80, 90, 100].to_vec();
        let (median, p95, p99) = percentiles(&s);
        assert_eq!(median, 55, "5th(50)+ceil(avg 50..60)=55");
        assert_eq!(p95, 100, "nearest rank p95 of 10 = rank ceil(9.5)=10");
        assert_eq!(p99, 100);

        // Odd count: median is the exact middle sample.
        let (m, p95o, p99o) = percentiles(&[5, 7, 100]);
        assert_eq!(m, 7);
        assert_eq!(p95o, 100, "p95 of 3 samples = rank ceil(2.85) → max sample");
        assert_eq!(p99o, 100);
    }

    #[test]
    fn environment_hash_ignores_recorded_at() {
        let mut e = Environment::capture(None);
        let first_hash = e.hash();
        e.recorded_at = Some("different-time".into());
        assert_eq!(e.hash(), first_hash, "time must not mint baselines");
        e.engine_version = "0.0.1".into();
        assert_ne!(e.hash(), first_hash, "engine version changes baseline key");
    }

    #[test]
    fn harness_never_runs_below_spec_minimums() {
        // Ask for 1 warm-up / 2 runs → still 3 + 10.
        let h = BenchmarkHarness::new(TestSceneId::TestC).with_counts(1, 2);
        assert_eq!(h.warmups, 3);
        assert_eq!(h.runs, 10);
    }

    #[test]
    fn benchmark_test_reports_spec_shape_for_a_frozen_scene() {
        let r = benchmark_test(TestSceneId::TestA).expect("benchmark runs");
        assert_eq!(r.scene, "test_a");
        assert!(r.p95_frame_us >= r.median_frame_us, "p95 ≥ median");
        assert!(r.p99_frame_us >= r.p95_frame_us, "p99 ≥ p95");
        assert!(r.peak_rss_bytes > 0, "real peak RSS recorded");
        assert!(r.export_frames_per_second.is_finite());
    }

    #[test]
    fn regression_requires_matched_environment_and_never_relaxes() {
        // CURRENT runs on env "bench-env"; the old approved baseline ran on
        // env "old-env" — different machines/environments.
        let current = BenchmarkResult {
            scene: "test_a".into(),
            median_frame_us: 100,
            p95_frame_us: 110,
            p99_frame_us: 120,
            peak_rss_bytes: 1,
            export_frames_per_second: 1.0,
            environment_hash: scenes::hash_bytes(b"bench-env"),
        };
        let mut base = current.clone();
        base.median_frame_us = 90;
        base.environment_hash = scenes::hash_bytes(b"old-env");
        let baseline = BenchmarkBaseline {
            result: base,
            environment: Environment::capture(None),
            approved_by: "reviewer".into(),
            approved_at: "2026-01-01T00:00:00Z".into(),
        };

        // Different env hash → EnvironmentChanged (new baseline, never compared).
        let env_changed = check_regression(&baseline, &current, 2.0).expect("checked");
        assert_eq!(env_changed, Some(Regression::EnvironmentChanged));

        // NOW a MATCHED baseline: same environment hash as current.
        let mut matched = baseline.clone();
        matched.result.environment_hash = scenes::hash_bytes(b"bench-env");
        // Same env hash: baseline 90us, current 100us = 1.11× — inside 2.0.
        assert!(check_regression(&matched, &current, 2.0).expect("ok").is_none());

        // Same hash, 3× slower, max 2.0 → named regression.
        let mut current3 = current.clone();
        current3.median_frame_us = 270;
        let reg = check_regression(&matched, &current3, 2.0).expect("checked");
        assert!(matches!(reg, Some(Regression::MatchedAndSlower(3.0))));
    }
}
