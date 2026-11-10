//! Frame/PCM comparison and approved baseline loading (spec §20.3 golden.rs,
//! §20.6, §20.11).
//!
//! Two comparison regimes by design:
//! - IMAGE goldens: cross-hardware byte tolerance is a DOCUMENTED, per-scene
//!   approved value (§20.13 manifests; ADR-0012 pinned-stack policy).
//! - PCM goldens: EXACT, tolerance 0 (§20.6).
//!
//! Baselines under `tests/golden/` are IMMUTABLE during test execution
//! (§20.8): this module loads them read-only and never rewrites. Golden
//! updates require explicit human review (§20.11) — a failed comparison
//! can never replace an approved baseline.

use anim_assets::hash::ContentHash;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One approved image frame: raw BGRA bytes + its blake3 content hash (the
/// hash is authoritative; bytes are the evidence).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageGolden {
    pub frame: u64,
    pub width: u32,
    pub height: u32,
    pub content_hash: ContentHash,
    /// Raw expected bytes (BGRA8, row-major). Optionally retained.
    #[serde(default)]
    pub bytes: Option<Vec<u8>>,
}

/// One approved PCM reference: exact interleaved i16 samples at 48 kHz.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PcmGolden {
    pub sample_frames: u64,
    pub channels: u16,
    pub content_hash: ContentHash,
    /// Exact expected samples (interleaved i16). Retained for exact verify.
    #[serde(default)]
    pub samples: Option<Vec<i16>>,
}

/// Frame comparison outcome (image regime keeps per-channel byte diff max).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameDiff {
    /// Maximum per-channel byte deviation observed (0 = pixel-exact).
    pub max_channel_delta: u8,
    pub mismatched_pixels: u64,
}

impl FrameDiff {
    pub fn within(&self, tolerance: u8) -> bool {
        self.max_channel_delta <= tolerance
    }
}

/// Compare a rendered BGRA8 frame against a golden ROW-INDEX-WISE with the
/// per-channel byte tolerance pinned by the scene manifest (§20.13).
///
/// Length/geometry mismatches are HARD failures — resizing is not a
/// tolerance concern and must never mask a pipeline bug.
pub fn compare_image_frame(
    golden: &ImageGolden,
    actual: &[u8],
    tolerance: u8,
) -> Result<FrameDiff, GoldenError> {
    let expected_len = golden.width as usize * golden.height as usize * 4;
    if actual.len() != expected_len {
        return Err(GoldenError::Size {
            frame: golden.frame,
            golden_bytes: expected_len,
            actual_bytes: actual.len(),
        });
    }
    let mut max_delta = 0u8;
    let mut mismatched = 0u64;
    let golden_px: &[[u8; 4]] =
        bytemuck::cast_slice(golden.bytes.as_deref().unwrap_or(&[]));
    let actual_px: &[[u8; 4]] = bytemuck::cast_slice(actual);
    for (win, a) in actual_px.iter().zip(golden_px.iter()) {
        let worst =
            win.iter().zip(a.iter()).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
        if worst > 0 {
            mismatched += 1;
        }
        max_delta = max_delta.max(worst);
    }
    let diff = FrameDiff { max_channel_delta: max_delta, mismatched_pixels: mismatched };
    if diff.max_channel_delta > tolerance {
        return Err(GoldenError::ImageBeyondTolerance {
            frame: golden.frame,
            delta: diff.max_channel_delta,
            tolerance,
        });
    }
    Ok(diff)
}

/// PCM comparison: EXACT (tolerance 0 by construction, spec §20.6). Length
/// mismatches and any sample divergence fail with the sample index of the
/// FIRST difference — no deviation is ever "within tolerance".
pub fn compare_pcm_exact(golden: &PcmGolden, actual: &[i16]) -> Result<(), GoldenError> {
    if golden.sample_frames as usize * golden.channels as usize != actual.len() {
        return Err(GoldenError::Size {
            frame: golden.sample_frames,
            golden_bytes: golden.sample_frames as usize * golden.channels as usize,
            actual_bytes: actual.len(),
        });
    }
    if let Some(expected) = &golden.samples {
        if let Some((i, (g, a))) =
            expected.iter().zip(actual.iter()).enumerate().find(|(_, (g, a))| g != a)
        {
            return Err(GoldenError::PcmSample {
                sample_index: i as u64,
                golden: *g,
                actual: *a,
            });
        }
    }
    // Hash regime covers baselines that ship hashes only: exact equality of
    // the blake3 content hash of the actual PCM.
    let actual_hash =
        ContentHash(crate::scenes::hash_bytes(bytemuck::cast_slice(actual)).0);
    if actual_hash != golden.content_hash {
        return Err(GoldenError::PcmHash {
            golden: golden.content_hash.full_hex(),
            actual: actual_hash.full_hex(),
        });
    }
    Ok(())
}

/// The frozen golden SET for one acceptance scene (test_a..test_e). Loaded
/// read-only; every field stays immutable during the run (§20.8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoldenSet {
    pub scene_id: crate::scenes::TestSceneId,
    pub image_frames: Vec<ImageGolden>,
    pub pcm: Vec<PcmGolden>,
}

impl GoldenSet {
    /// Load the approved baseline from `tests/golden/<dir_name>/golden.json`
    /// (read-only; spec §20.3 golden directory).
    pub fn load(
        golden_root: &Path,
        scene_id: crate::scenes::TestSceneId,
    ) -> Result<GoldenSet, GoldenError> {
        let path = golden_root.join(scene_id.dir_name()).join("golden.json");
        let bytes = std::fs::read(&path).map_err(|e| {
            GoldenError::Io(path.to_string_lossy().into_owned(), e.to_string())
        })?;
        serde_json::from_slice(&bytes).map_err(|e| {
            GoldenError::Deserialize(path.to_string_lossy().into_owned(), e.to_string())
        })
    }

    /// Look up one approved image frame baseline.
    pub fn image_frame(&self, frame: u64) -> Option<&ImageGolden> {
        self.image_frames.iter().find(|g| g.frame == frame)
    }
}

/// Typed golden errors (§20.11: typed and explicit; a failed comparison
/// cannot rewrite anything).
#[derive(Debug, thiserror::Error)]
pub enum GoldenError {
    #[error("geometry mismatch at frame {frame}: golden {golden_bytes} bytes vs actual {actual_bytes}")]
    Size { frame: u64, golden_bytes: usize, actual_bytes: usize },
    #[error(
        "exact PCM mismatch at sample {sample_index}: golden {golden} vs actual {actual}"
    )]
    PcmSample { sample_index: u64, golden: i16, actual: i16 },
    #[error("exact PCM hash mismatch: golden {golden} vs actual {actual}")]
    PcmHash { golden: String, actual: String },
    #[error("baseline beyond approved tolerance: frame {frame} max channel delta {delta} > tolerance {tolerance}")]
    ImageBeyondTolerance { frame: u64, delta: u8, tolerance: u8 },
    #[error("golden file {0} unreadable: {1}")]
    Io(String, String),
    #[error("golden file {0} unparseable: {1}")]
    Deserialize(String, String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_within_tolerance_passes_and_beyond_fails_typed() {
        let golden = ImageGolden {
            frame: 0,
            width: 2,
            height: 1,
            content_hash: ContentHash([0u8; 32]),
            bytes: Some(vec![10, 10, 10, 255, 20, 20, 20, 255]),
        };
        // Delta of exactly 2 per channel is approved at tolerance 2.
        let ok = compare_image_frame(&golden, &[12, 12, 12, 255, 22, 20, 20, 255], 2)
            .expect("within tolerance");
        assert!(ok.within(2));
        assert_eq!(ok.max_channel_delta, 2);
        // Delta 3 fails typed, naming frame, delta and tolerance.
        let err = compare_image_frame(&golden, &[13, 10, 10, 255, 20, 20, 20, 255], 2)
            .expect_err("delta 3 > 2");
        assert!(matches!(
            err,
            GoldenError::ImageBeyondTolerance { frame: 0, delta: 3, tolerance: 2 }
        ));
    }

    #[test]
    fn image_geometry_mismatch_is_hard_failure() {
        let golden = ImageGolden {
            frame: 3,
            width: 4,
            height: 4,
            content_hash: ContentHash([0u8; 32]),
            bytes: None,
        };
        let err =
            compare_image_frame(&golden, &[0u8; 4 * 5 * 4], 2).expect_err("wrong size");
        assert!(matches!(err, GoldenError::Size { frame: 3, .. }));
    }

    #[test]
    fn pcm_exact_first_divergence_is_named() {
        let samples = vec![100i16, -50, 3];
        let hash = crate::scenes::hash_bytes(bytemuck::cast_slice(&samples));
        let golden = PcmGolden {
            sample_frames: 3,
            channels: 1,
            content_hash: hash,
            samples: Some(samples.clone()),
        };
        assert!(compare_pcm_exact(&golden, &samples).is_ok());
        let mut bad = samples.clone();
        bad[1] = -49;
        let err = compare_pcm_exact(&golden, &bad).expect_err("changed sample");
        assert!(matches!(
            err,
            GoldenError::PcmSample { sample_index: 1, golden: -50, actual: -49 }
        ));
    }

    #[test]
    fn pcm_length_mismatch_is_typed_not_tolerated() {
        let golden = PcmGolden {
            sample_frames: 2,
            channels: 2,
            content_hash: ContentHash([7u8; 32]),
            samples: None,
        };
        let err = compare_pcm_exact(&golden, &[0i16; 4]).expect_err("hash empty too");
        assert!(matches!(err, GoldenError::PcmHash { .. }));
    }
}
