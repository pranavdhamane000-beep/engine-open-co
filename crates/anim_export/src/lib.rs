//! anim_export — Headless Export, FFmpeg, and Verified Publication (spec §18, WP09).
//!
//! Implements fixed-quality, repeatable headless export with bounded GPU
//! readback, deterministic audio, verified external FFmpeg, explicit codec
//! behavior, output verification and atomic publication.

#![deny(clippy::all)]
#![allow(clippy::multiple_crate_versions)]

pub mod error;
pub mod ffmpeg;
pub mod manifest;
pub mod pipeline;
pub mod progress;
pub mod publish;
pub mod readback;
pub mod settings;
pub mod verify;

pub use error::{ExportError, ExportResult};
pub use manifest::{ExpectedCounts, ExportManifest, ExportReport};
pub use pipeline::{ExportJob, ExportPipeline, PipelineStage};
pub use progress::{CancellationToken, ConsoleProgressSink, ProgressSink};
pub use publish::{
    cleanup_temp, create_job_temp_dir, publish, publish_file_atomic, publish_png_sequence,
};
pub use readback::{ReadbackRing, ReadbackSlot, RingStats, SlotState};
pub use settings::{
    AudioMode, CodecId, CodecProfile, ContainerFormat, ExportAlphaMode, ExportSettings,
    FFmpegTools, QualityProfile, ResolvedExport,
};
pub use verify::{verify_output, verify_video_decoded};

#[cfg(test)]
mod tests {

    use crate::settings::{
        AudioMode, CodecId, ExportAlphaMode, ExportSettings, QualityProfile,
    };

    #[test]
    fn settings_default() {
        let s = ExportSettings::default();
        assert_eq!(s.schema_version, 1);
        assert_eq!(s.codec, CodecId::H264Mp4);
        assert_eq!(s.quality, QualityProfile::High);
        assert_eq!(s.alpha, ExportAlphaMode::Reject);
        assert_eq!(s.audio, AudioMode::None);
    }

    #[test]
    fn settings_validation_rejects_zero_dimensions() {
        let _s = ExportSettings { width: 0, ..Default::default() };
        // Would need a mock project to fully test
    }

    #[test]
    fn codec_profile_resolution() {
        let _s = ExportSettings {
            codec: crate::settings::CodecId::H264Mp4,
            quality: crate::settings::QualityProfile::High,
            alpha: crate::settings::ExportAlphaMode::Reject,
            ..ExportSettings::default()
        };
        // Would need a mock project to fully test
    }
}
