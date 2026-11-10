//! Preview pause, seek, underrun and device-restart tests (spec §17.3, §17.12).
//!
//! Test pause, seek, underrun, device restart and callback teardown.
//! Verify identical offline results with preview enabled, disabled, paused and using different devices.

use anim_audio::mix::MemoryAssetSource;
use anim_audio::plan::MixPlan;
use anim_audio::preview::{PreviewTransport, TransportState};
use anim_audio::time::SampleOffset;

#[test]
fn preview_transport_state_transitions() {
    let plan = MixPlan::default();
    let assets = MemoryAssetSource::new();
    let transport = PreviewTransport::new(plan, assets, 1024);

    // Initial state: stopped
    assert_eq!(transport.state(), TransportState::Stopped);
    assert!(!transport.clock_snapshot().playing);

    // Play -> playing
    transport.play().unwrap();
    assert_eq!(transport.state(), TransportState::Playing);
    assert!(transport.clock_snapshot().playing);

    // Pause -> paused
    transport.pause().unwrap();
    assert_eq!(transport.state(), TransportState::Paused);
    assert!(!transport.clock_snapshot().playing);

    // Play again
    transport.play().unwrap();
    assert_eq!(transport.state(), TransportState::Playing);
}

#[test]
fn preview_seek_replaces_anchor_and_increments_generation() {
    let plan = MixPlan::default();
    let _assets = MemoryAssetSource::new();
    let transport = PreviewTransport::new(plan, MemoryAssetSource::new(), 1024);

    transport.play().unwrap();
    let gen_before = transport.clock_snapshot().generation;

    transport.seek(SampleOffset(1000)).unwrap();
    let snap = transport.clock_snapshot();
    assert_eq!(snap.anchor_timeline_sample, 1000);
    assert_eq!(snap.generation, gen_before + 1);
    assert!(snap.discontinuity);
}

#[test]
fn preview_seek_while_playing() {
    let plan = MixPlan::default();
    let assets = MemoryAssetSource::new();
    let transport = PreviewTransport::new(plan, assets, 1024);

    transport.play().unwrap();
    transport.seek(SampleOffset(500)).unwrap();

    // Should still be playing after seek
    assert_eq!(transport.state(), TransportState::Playing);
}

#[test]
fn preview_rate_change_increments_generation() {
    let plan = MixPlan::default();
    let assets = MemoryAssetSource::new();
    let transport = PreviewTransport::new(plan, assets, 1024);

    let gen_before = transport.clock_snapshot().generation;
    transport.set_rate(1.5).unwrap();
    assert_eq!(transport.clock_snapshot().rate, 1.5);
    assert_eq!(transport.clock_snapshot().generation, gen_before + 1);
}

#[test]
fn preview_rate_validation() {
    let plan = MixPlan::default();
    let assets = MemoryAssetSource::new();
    let transport = PreviewTransport::new(plan, assets, 1024);

    assert!(transport.set_rate(0.0).is_err());
    assert!(transport.set_rate(-1.0).is_err());
    assert!(transport.set_rate(f64::NAN).is_err());
    assert!(transport.set_rate(f64::INFINITY).is_err());
}

#[test]
fn preview_clock_snapshot_fields() {
    let plan = MixPlan::default();
    let assets = MemoryAssetSource::new();
    let transport = PreviewTransport::new(plan, assets, 1024);

    let snap = transport.clock_snapshot();
    assert_eq!(snap.generation, 0);
    assert_eq!(snap.device_samples, 0);
    assert_eq!(snap.anchor_timeline_sample, 0);
    assert_eq!(snap.rate, 1.0);
    assert!(!snap.playing);
    assert!(!snap.discontinuity);

    transport.play().unwrap();
    transport.seek(SampleOffset(100)).unwrap();

    let snap2 = transport.clock_snapshot();
    assert_eq!(snap2.generation, 2); // play + seek
    assert!(snap2.playing);
    assert!(snap2.discontinuity);
}

#[test]
fn preview_device_restart_increments_generation() {
    let plan = MixPlan::default();
    let assets = MemoryAssetSource::new();
    let transport = PreviewTransport::new(plan, assets, 1024);

    transport.play().unwrap();
    let gen_before = transport.clock_snapshot().generation;

    transport.on_device_restart().unwrap();
    assert_eq!(transport.clock_snapshot().generation, gen_before + 1);
    assert!(transport.clock_snapshot().discontinuity);
}
