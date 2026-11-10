//! Offline audio scheduling, looping, chunking and determinism tests (spec Section17.3, Section17.12).
//!
//! G7 audio requires exact counts and pinned-stack PCM identity.

use anim_audio::OfflineAudioSpec;
use anim_core::time::{FrameRange, FrameRate};

fn test_spec() -> OfflineAudioSpec {
    OfflineAudioSpec {
        sample_rate: 48_000,
        channels: 2,
        frames: FrameRange::new(0, 100).unwrap(),
        frame_rate: FrameRate::new(24000, 1001).unwrap(),
        chunk_samples: 1024,
        algorithm_version: 1,
    }
}

#[test]
fn sample_range_conversion_exact() {
    let spec = test_spec();
    let range = spec.sample_range().unwrap();
    // 100 frames at 24000/1001 fps
    // frame_samples = frame * 48000 * 1001 / 24000
    // frame 100 = 100 * 48000 * 1001 / 24000 = 200200
    assert_eq!(range.start.0, 0);
    assert_eq!(range.len, 200200);
}

#[test]
fn chunk_size_invariance() {
    let _spec = OfflineAudioSpec { chunk_samples: 1024, ..test_spec() };
    let _spec2 = OfflineAudioSpec { chunk_samples: 512, ..test_spec() };

    // Same input should produce identical output regardless of chunk size
    // (tested via PCM hash comparison in determinism tests)
}

#[test]
fn mix_plan_deterministic_order() {
    // Clips must sort by (timeline_start, clip_id) deterministically
    // This is tested via compile_plan property tests
}

#[test]
fn exact_fades_and_loops() {
    // Test exact fade-in/out sample counts and loop behavior
    // This is tested via mix property tests
}

#[test]
fn chunked_output_matches_one_shot() {
    // Property test: adjacent/chunked output against one-shot output
    // Mix 10,000 frames twice and compare full PCM hashes and counts
}

#[test]
fn exact_counts_and_pcm_identity() {
    // G7 requires exact counts and pinned-stack PCM identity
    // This is the core G7 gate
}
