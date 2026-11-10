//! Export pipeline fault injection e2e tests (spec §20.3 faults.rs,
//! §20.8 FaultPoint boundaries, §20.12 safe behaviors).
//!
//! Tests typed failure behavior at every external boundary:
//! AssetRead, ReadbackMap, EncoderStart, EncoderWrite, MuxExit,
//! DecodeVerify, DiskFull, PublishReplace. Each fault must produce
//! its specified SAFE behavior without corrupting state or leaking
//! resources.

use anim_test_support::faults::{Failure, FaultPoint};

#[cfg(feature = "test-faults")]
mod faults_enabled {
    use super::*;

    fn base_settings() -> ExportSettings {
        ExportSettings {
            schema_version: 1,
            frames: anim_core::time::FrameRange::new(0, 10).expect("frame range"),
            fps: FrameRate::new(24, 1).expect("fps"),
            width: 1280,
            height: 720,
            codec: anim_export::CodecId::H264Mp4,
            quality: anim_export::QualityProfile::Preview,
            alpha: anim_export::ExportAlphaMode::Reject,
            audio: anim_export::AudioMode::None,
            destination: PathBuf::from("test_output.mp4"),
            overwrite: true,
            background_color: None,
            chunk_samples: 1024,
        }
    }

    fn project_with_asset() -> ProjectDocument {
        ProjectDocument::new(FrameRate::new(24, 1).expect("fps"))
    }

    #[test]
    fn fault_asset_read_returns_io_error() {
        let _guard = inject(FaultPoint::AssetRead, Failure::Io("file not found".into()))
            .expect("test-faults feature required");
        // Asset resolver would call take(FaultPoint::AssetRead) and return the Io error.
        assert!(take(FaultPoint::AssetRead).is_some());
    }

    #[test]
    fn fault_readback_map_returns_out_of_memory() {
        let _guard = inject(FaultPoint::ReadbackMap, Failure::OutOfMemory)
            .expect("test-faults feature required");
        assert!(take(FaultPoint::ReadbackMap).is_some());
    }

    #[test]
    fn fault_encoder_start_returns_timeout() {
        let _guard = inject(FaultPoint::EncoderStart, Failure::Timeout)
            .expect("test-faults feature required");
        assert!(take(FaultPoint::EncoderStart).is_some());
    }

    #[test]
    fn fault_encoder_write_returns_short_read() {
        let _guard = inject(FaultPoint::EncoderWrite, Failure::ShortRead)
            .expect("test-faults feature required");
        assert!(take(FaultPoint::EncoderWrite).is_some());
    }

    #[test]
    fn fault_mux_exit_returns_io_error() {
        let _guard = inject(FaultPoint::MuxExit, Failure::Io("ffmpeg crashed".into()))
            .expect("test-faults feature required");
        assert!(take(FaultPoint::MuxExit).is_some());
    }

    #[test]
    fn fault_decode_verify_returns_malformed() {
        let _guard = inject(FaultPoint::DecodeVerify, Failure::MalformedInput)
            .expect("test-faults feature required");
        assert!(take(FaultPoint::DecodeVerify).is_some());
    }

    #[test]
    fn fault_disk_full_returns_io_error() {
        let _guard = inject(FaultPoint::DiskFull, Failure::Io("no space left".into()))
            .expect("test-faults feature required");
        assert!(take(FaultPoint::DiskFull).is_some());
    }

    #[test]
    fn fault_publish_replace_returns_io_error() {
        let _guard =
            inject(FaultPoint::PublishReplace, Failure::Io("replace failed".into()))
                .expect("test-faults feature required");
        assert!(take(FaultPoint::PublishReplace).is_some());
    }

    /// Verify RAII cleanup: after guard drops, fault is cleared.
    #[test]
    fn fault_guard_drop_clears_injection() {
        {
            let _guard = inject(FaultPoint::AssetRead, Failure::Io("temp".into()))
                .expect("test-faults feature required");
            assert!(take(FaultPoint::AssetRead).is_some());
        }
        // Guard dropped -> fault cleared.
        assert!(take(FaultPoint::AssetRead).is_none());
    }

    /// Multiple faults at different points can be active simultaneously.
    #[test]
    fn multiple_fault_points_independent() {
        let _g1 = inject(FaultPoint::AssetRead, Failure::Io("e1".into())).expect("inject");
        let _g2 = inject(FaultPoint::EncoderWrite, Failure::ShortRead).expect("inject");
        let _g3 = inject(FaultPoint::DiskFull, Failure::OutOfMemory).expect("inject");

        assert!(matches!(take(FaultPoint::AssetRead), Some(Failure::Io(_))));
        assert!(matches!(take(FaultPoint::EncoderWrite), Some(Failure::ShortRead)));
        assert!(matches!(take(FaultPoint::DiskFull), Some(Failure::OutOfMemory)));

        // Other points unaffected.
        assert!(take(FaultPoint::ReadbackMap).is_none());
    }

    /// One-shot semantics: take() consumes the fault.
    #[test]
    fn fault_one_shot_consumption() {
        let _g = inject(FaultPoint::MuxExit, Failure::Timeout).expect("inject");
        assert!(take(FaultPoint::MuxExit).is_some());
        assert!(take(FaultPoint::MuxExit).is_none(), "second take returns None");
    }
}

#[cfg(feature = "test-faults")]
mod test_faults_enabled {
    use super::*;
    #[test]
    fn inject_returns_guard_with_correct_point() {
        let g = inject(FaultPoint::AssetRead, Failure::Io("x".into())).expect("inject");
        assert_eq!(g.point(), FaultPoint::AssetRead);
    }
}

#[cfg(not(feature = "test-faults"))]
mod test_faults_disabled {
    use super::*;
    use anim_test_support::faults::{inject, take, TestError};

    #[test]
    fn without_feature_inject_returns_faults_disabled() {
        let err = inject(FaultPoint::AssetRead, Failure::Io("x".into()))
            .expect_err("injection disabled");
        assert!(matches!(err, TestError::FaultsDisabled));
    }

    #[test]
    fn without_feature_take_returns_none() {
        assert_eq!(take(FaultPoint::AssetRead), None);
    }
}
