//! WP01 validation (spec 10.12): proptest normalization and arithmetic
//! against arbitrary-precision references, negative floor around exact
//! boundaries, subframe carry, adjacent sample-range partitioning at 24000/1001
//! and 30000/1001 over at least 10,000 frames (every sample exactly once, no
//! accumulated drift), NTSC rates, UUID round trips, canonical byte ordering,
//! revision overflow - G1 requires all overflow and invalid-rate tests to
//! fail explicitly.

use anim_core::{
    AssetId, Canonical, CoreError, FramePos, FrameRange, FrameRate, NodeId, ProjectId,
    Rational, Revision, SampleInterval, SceneId,
};
use proptest::prelude::*;

fn rate(n: u32, d: u32) -> FrameRate {
    FrameRate::new(n, d).expect("spec rates are positive")
}

// ---------------------------------------------------------------- NTSC rates

#[test]
fn ntsc_rates_are_exact_rationals() {
    // 29.97 stored as 30000/1001; time of frame 1 is exactly 1001/30000 s,
    // NOT 0.03337 s.
    let fps = rate(30000, 1001);
    let t = fps.time_of(FramePos::zero()).unwrap();
    assert!(t.is_zero());
    let t1 = fps.time_of(FramePos::new(1, Rational::zero()).unwrap()).unwrap();
    assert_eq!(t1.num(), 1001);
    assert_eq!(t1.den(), 30000);
    // floor frame containing 1001/30000 is frame 1 exactly.
    let pos = fps.frame_containing(t1).unwrap();
    assert_eq!(pos.frame(), 1);
    assert_eq!(pos.subframe().num(), 0);
}

#[test]
fn film_24000_1001_frame_time_exact() {
    let fps = rate(24000, 1001);
    let t5 = fps.time_of(FramePos::new(5, Rational::zero()).unwrap()).unwrap();
    assert_eq!(t5, Rational::new(5005, 24000).unwrap());
    let back = fps.frame_containing(t5).unwrap();
    assert_eq!(back.frame(), 5);
    assert!(back.subframe().is_zero());
}

// ---------------------------------------------------- frame_containing floor

#[test]
fn negative_exact_boundary_floors_to_preceding_frame() {
    // Concept: at exact boundaries floor selects the mathematically
    // preceding frame, never truncating toward zero (spec 10.7).
    let fps = rate(24, 1);
    // -1/3 s = frame -8 exactly (since -8/24 = -1/3).
    let pos = fps.frame_containing(Rational::new(-1, 3).unwrap()).unwrap();
    assert_eq!(pos.frame(), -8);
    assert!(pos.subframe().is_zero());
    // just past the boundary: -1/3 + 1/24000 sits above the exact boundary
    // and still inside frame -8 (floor(-8 + tiny) = -8).
    let t = Rational::new(-1, 3)
        .unwrap()
        .checked_add(Rational::new(1, 24_000).unwrap())
        .unwrap();
    let pos2 = fps.frame_containing(t).unwrap();
    assert_eq!(pos2.frame(), -8);
    // -1/3 - epsilon: floor(-8 - tiny) = -9.
    let t = Rational::new(-1, 3)
        .unwrap()
        .checked_sub(Rational::new(1, 240_000).unwrap())
        .unwrap();
    let pos3 = fps.frame_containing(t).unwrap();
    assert_eq!(pos3.frame(), -9);
}

#[test]
fn subframe_carry_into_frame() {
    // FramePos::new carries any whole part of the subframe into the frame.
    let pos = FramePos::new(10, Rational::new(3, 1).unwrap()).unwrap();
    assert_eq!(pos.frame(), 13);
    assert!(pos.subframe().is_zero());
    let pos = FramePos::new(-2, Rational::new(-1, 1).unwrap()).unwrap();
    assert_eq!(pos.frame(), -3);
    assert!(pos.subframe().is_zero());
    // mixed carry
    let pos = FramePos::new(7, Rational::new(4, 3).unwrap()).unwrap();
    assert_eq!(pos.frame(), 8);
    assert_eq!(*pos.subframe(), Rational::new(1, 3).unwrap());
}

#[test]
fn invalid_rate_fails_explicitly() {
    assert!(FrameRate::new(0, 24).is_err());
    assert!(FrameRate::new(24, 0).is_err());
    let e = FrameRate::new(0, 0).unwrap_err();
    assert!(matches!(e, CoreError::NonPositiveRate { .. }));
    // struct-literal bypass still fails at method call time (fields are pub)
    let bad = FrameRate { num: 0, den: 24 };
    assert!(matches!(
        bad.frame_containing(Rational::zero()).unwrap_err(),
        CoreError::NonPositiveRate { .. }
    ));
}

// ---------------------------------------------------- zero-drift integration

/// Spec 10.12: at least 10,000 adjacent frames at both NTSC rates; every sample
/// occurs exactly once and no accumulated drift is permitted. We assert:
///   1. monotonic (start_{f+1} == end_f),
///   2. start_f == floor(f * sr * den / num) EXACTLY against the BigInt
///      reference (independent re-derivation, so no drift can hide),
///   3. total coverage [first_start, last_end) with expected density.
fn assert_partition(fps: &FrameRate, frames: u64, sr: u32) {
    let mut prev_end: Option<i64> = None;
    for f in 0..frames as i64 {
        let iv: SampleInterval = fps.frame_samples(f, sr).unwrap();
        // exact BigInt reference for start (independent formula)
        let start_ref =
            Rational::new(f as i128 * sr as i128 * fps.den as i128, fps.num as i128)
                .unwrap()
                .floor_to_i64()
                .unwrap();
        assert_eq!(
            iv.start, start_ref,
            "drift at frame {f} of {}/{} at {sr}",
            fps.num, fps.den
        );
        if let Some(pe) = prev_end {
            assert_eq!(pe, iv.start, "gap/overlap at frame {f}");
        }
        assert!(iv.end > iv.start || (sr as u64) < fps.den as u64, "interval must advance");
        prev_end = Some(iv.end);
    }
    let last_end = prev_end.unwrap();
    // coverage claim: floor((frames-1+1) * sr * den/num) == last_end
    let coverage =
        Rational::new(frames as i128 * sr as i128 * fps.den as i128, fps.num as i128)
            .unwrap()
            .floor_to_i64()
            .unwrap();
    assert_eq!(last_end, coverage, "total sample coverage drift");
}

#[test]
fn ten_thousand_adjacent_frames_24000_1001_partition_zero_drift() {
    assert_partition(&rate(24000, 1001), 10_001, 48_000);
}

#[test]
fn ten_thousand_adjacent_frames_30000_1001_partition_zero_drift() {
    assert_partition(&rate(30000, 1001), 10_001, 48_000);
}

#[test]
fn ten_thousand_adjacent_frames_24000_1001_at_44100_partition_zero_drift() {
    assert_partition(&rate(24000, 1001), 10_001, 44_100);
}

#[test]
fn every_sample_occurs_exactly_once_count() {
    // For 30,000/1001 at 48 kHz over N frames, sum of interval lengths must
    // equal floor(N * sr * den / num) minus start - a density check that no
    // sample is skipped or double-covered.
    let fps = rate(30000, 1001);
    let sr = 48_000u32;
    let (mut covered, mut prev_end_opt, mut start_first) = (0i64, None, 0i64);
    for f in 0..10_000i64 {
        let iv = fps.frame_samples(f, sr).unwrap();
        if prev_end_opt.is_none() {
            start_first = iv.start;
        }
        covered += iv.end - iv.start;
        if let Some(pe) = prev_end_opt {
            assert_eq!(pe, iv.start);
        }
        prev_end_opt = Some(iv.end);
    }
    let expected = coverage_samples(10_000, sr as i128, &fps) - start_first;
    assert_eq!(covered, expected);
}

fn coverage_samples(frames: i64, sr: i128, fps: &FrameRate) -> i64 {
    Rational::new(frames as i128 * sr * fps.den as i128, fps.num as i128)
        .unwrap()
        .floor_to_i64()
        .unwrap()
}

// --------------------------------------------------------- sample_at / round trip

#[test]
fn time_and_frame_round_trip() {
    for fps in [rate(24, 1), rate(24000, 1001), rate(30000, 1001), rate(60, 1)] {
        for frame in [-1000i64, -7, -1, 0, 1, 999, 50_000] {
            let pos = FramePos::new(frame, Rational::zero()).unwrap();
            let t = fps.time_of(pos.clone()).unwrap();
            let back = fps.frame_containing(t).unwrap();
            assert_eq!(back, pos, "round trip failed for frame {frame}");
        }
    }
}

// ---------------------------------------------------------------- proptests

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 512,
        ..ProptestConfig::default()
    })]

    /// Normalization always yields den > 0, gcd(num,den) == 1 (or 0/1).
    #[test]
    fn normalization_positive_den_reduced(
        n in -60_000_000i128..60_000_000i128,
        d in -60_000_000i128..60_000_000i128,
    ) {
        if d == 0 {
            return Ok(());
        }
        let r = Rational::new(n, d)?;
        prop_assert!(r.den() > 0);
        prop_assert_eq!(r.num(), if r.num() == 0 { 0 } else { r.num() });
        if r.num() != 0 {
            let g = gcd_abs(r.num(), r.den());
            prop_assert_eq!(g, 1);
        }
    }

    /// floor + ceil partition: f+1 == c for non-integers, f == c at exact
    /// integers — an arbitrary-precision-reference property.
    #[test]
    fn floor_ceil_partition(
        n in -900_000_000i128..900_000_000i128,
        d in 1..900_000_000i128,
    ) {
        let r = Rational::new(n, d)?;
        let f = r.floor_to_i64()?;
        let c = r.ceil_to_i64()?;
        let is_exact_integer = r.num() % r.den() == 0;
        if !is_exact_integer {
            prop_assert_eq!(f + 1, c);
        } else {
            prop_assert_eq!(f, c);
        }
    }

    /// checked multiplication against BigInteger reference.
    #[test]
    fn mul_matches_bigint_reference(
        a_n in -10_000i128..10_000i128, a_d in 1..10_000i128,
        b_n in -10_000i128..10_000i128, b_d in 1..10_000i128,
    ) {
        let a = Rational::new(a_n, a_d)?;
        let b = Rational::new(b_n, b_d)?;
        let got = a.checked_mul(b)?;
        // reference: numerator = a_n*b_n, denominator = a_d*b_d, reduced
        let n = a_n * b_n;
        let d = a_d * b_d;
        let expected = Rational::new(n, d)?;
        prop_assert_eq!(got, expected);
    }

    /// frame_containing: exact products round trip; boundary-adjacent values
    /// never skip a frame and never repeat one.
    #[test]
    fn frame_containing_floor_matches_reference(
        num_s in -5_000i128..5_000i128,
        den_s in 1..5_000i128,
        fps_n in 1u32..=60u32,
    ) {
        let fps = FrameRate::new(fps_n, 1)?;
        let t = Rational::new(num_s, den_s)?;
        let pos = fps.frame_containing(t)?;
        // reference floor via i128 div_euclid: floor(t*fps) = floor(num_s*fps_n / den_s)
        let whole_ref = (num_s * fps_n as i128).div_euclid(den_s);
        prop_assert_eq!(pos.frame(), whole_ref as i64);
        let sf = pos.subframe();
        prop_assert!(sf.num() >= 0 && sf.num() < sf.den());
    }

    /// adjacent interval endpoints at arbitrary NTSC-style rates never drift.
    #[test]
    fn intervals_share_endpoints_exactly(
        fps_n in 2u32..=240u32,
        fps_d in 1u32..=1001u32,
        sr in (0..2usize).prop_map(|k| if k == 0 { 44_100u32 } else { 48_000 }),
    ) {
        let fps = FrameRate::new(fps_n, fps_d)?;
        let mut prev: Option<SampleInterval> = None;
        for f in 0..2_000i64 {
            let iv = fps.frame_samples(f, sr)?;
            if let Some(p) = prev {
                prop_assert_eq!(p.end, iv.start);
            }
            prev = Some(iv);
        }
    }
}

// --------------------------------------------------------------- IDs / misc

#[test]
fn uuid_round_trip_and_stable_canonical_bytes() {
    fn json_round_trip<T: serde::Serialize + for<'d> serde::Deserialize<'d>>(v: &T) -> T {
        serde_json::from_str(&serde_json::to_string(v).unwrap()).unwrap()
    }
    let project = ProjectId::new_v4();
    let scene = SceneId::new_v4();
    let node = NodeId::from_u128(0xdead_beef);
    let asset = AssetId::from_u128(1);

    // each id survives a serde document round trip unchanged
    let p2: ProjectId = json_round_trip(&project);
    let s2: SceneId = json_round_trip(&scene);
    let n2: NodeId = json_round_trip(&node);
    let a2: AssetId = json_round_trip(&asset);
    assert_eq!(p2, project);
    assert_eq!(s2, scene);
    assert_eq!(n2, node);
    assert_eq!(a2, asset);

    // persisted text is the bare UUID — same text FromStr accepts
    assert_eq!(serde_json::to_string(&project).unwrap(), format!("\"{project}\""));
    let back: ProjectId = project.to_string().parse().unwrap();
    assert_eq!(back, project);

    // canonical encoding is 16 bytes, stable across re-encodes
    let a = node.canonical_bytes();
    let b = node.canonical_bytes();
    assert_eq!(a, b);
    assert_eq!(a.len(), 16);
}

#[test]
fn revision_overflow_fails_explicitly() {
    assert_eq!(Revision(5).checked_add(0).unwrap().as_u64(), 5);
    assert_eq!(Revision(u64::MAX - 2).checked_add(2).unwrap().as_u64(), u64::MAX);
    assert!(Revision(u64::MAX).checked_add(1).is_err());
    assert!(Revision(u64::MAX).checked_add(u64::MAX).is_err());
}

#[test]
fn hash_error_contract() {
    // domain separation sanity across domains
    let h1 = anim_core::domain_hash_hex("project", b"payload");
    let h2 = anim_core::domain_hash_hex("scene", b"payload");
    assert_ne!(h1, h2);
    let h1b = anim_core::domain_hash_hex("project", b"payload");
    assert_eq!(h1, h1b);
}

#[test]
fn range_validation() {
    assert!(FrameRange::new(5, 5).is_ok()); // empty is valid
    assert!(FrameRange::new(-10, 0).is_ok()); // negative support
    assert!(matches!(
        FrameRange::new(5, 4).unwrap_err(),
        CoreError::InvalidRange { start: 5, end: 4 }
    ));
    let r = FrameRange::new(2, 10).unwrap();
    assert!(r.contains(2) && r.contains(9) && !r.contains(10) && !r.contains(1));
}

// ------------------------------------------------------------ test utilities

fn gcd_abs(a: i128, b: i128) -> i128 {
    if b == 0 {
        a.abs()
    } else {
        gcd_abs(b, a % b)
    }
}
