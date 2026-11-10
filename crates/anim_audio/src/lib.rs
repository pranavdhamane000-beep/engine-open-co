//! anim_audio - Deterministic Offline Audio and Preview Clock (spec Section17, WP08).
//!
//! Produces deterministic stereo 48 kHz PCM for export using exact integer
//! sample positions. Provides an independent real-time preview driven by the
//! audio output stream, without allowing audio-device state, callback timing
//! or preview transport to alter offline output.

#![deny(clippy::all)]
#![allow(clippy::multiple_crate_versions)]

pub mod decode;
pub mod error;
pub mod mix;
pub mod offline;
pub mod output;
pub mod plan;
pub mod preview;
pub mod resample;
pub mod time;
pub mod wav;

pub use decode::{decode_asset, DecodeCacheKey, DecodedAsset};
pub use error::{AudioError, AudioResult, PreviewError, PreviewResult};
pub use mix::{mix_range, FadeCurve, MemoryAssetSource, PcmAssetSource};
pub use offline::{render_offline, OfflineRenderer};
pub use plan::{compile_plan, MixPlan};
pub use preview::{PreviewClockSnapshot, PreviewTransport, TransportState};
pub use resample::{
    resample_to_canonical, StreamingResampler, OUTPUT_CHANNELS, OUTPUT_SAMPLE_RATE,
};
pub use time::{OfflineAudioSpec, PlannedClip, SampleOffset, SampleRange};
pub use wav::{write_wav, write_wav_streaming};

#[cfg(feature = "preview-audio")]
pub use output::{PreviewClockSnapshot as OutputPreviewClockSnapshot, PreviewOutput};

#[cfg(test)]
mod tests {

    use crate::time::{OfflineAudioSpec, SampleOffset, SampleRange};

    #[test]
    fn sample_offset_arithmetic() {
        let a = SampleOffset(100);
        assert_eq!(a.checked_add(50).unwrap(), SampleOffset(150));
        assert_eq!(a.checked_sub(30).unwrap(), SampleOffset(70));
        assert!(a.checked_sub(200).is_err());
    }

    #[test]
    fn sample_range_operations() {
        let r = SampleRange::new(SampleOffset(100), 50);
        assert_eq!(r.end(), SampleOffset(150));
        assert!(r.contains(SampleOffset(120)));
        assert!(!r.contains(SampleOffset(99)));
        assert!(!r.contains(SampleOffset(150)));

        let r2 = SampleRange::new(SampleOffset(120), 40);
        let inter = r.intersection(&r2).unwrap();
        assert_eq!(inter.start, SampleOffset(120));
        assert_eq!(inter.len, 30);
    }

    #[test]
    fn offline_spec_default() {
        let spec = OfflineAudioSpec::default();
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.chunk_samples, 1024);
        assert_eq!(spec.algorithm_version, 1);
    }

    #[test]
    fn frame_to_sample_conversion() {
        use anim_core::time::{FrameRange, FrameRate};
        let spec = OfflineAudioSpec::new(
            FrameRange::new(0, 100).unwrap(),
            FrameRate::new(24000, 1001).unwrap(),
        );
        let range = spec.sample_range().unwrap();
        // 100 frames at 24000/1001 fps = 100 * 48000 * 1001 / 24000 = 200200 samples
        assert_eq!(range.len, 200200);
    }
}
