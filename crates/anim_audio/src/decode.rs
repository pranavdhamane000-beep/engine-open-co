//! Source PCM decode and canonical channel/rate metadata (spec Section17.3, Section17.7).
//!
//! Uses symphonia for enabled source formats. Decoded PCM is cached by
//! (content_hash, decoder_version, resampler_version, settings_hash).
//! Streaming resampler state is private to an asset decode session.

use crate::error::{AudioError, AudioResult};
use anim_assets::{hash::ContentHash, AssetResolver};
use std::fs::File;
use std::io::Cursor;
use std::io::Read;
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Decoded PCM asset with canonical metadata (spec Section17.3, Section17.7).
#[derive(Debug, Clone)]
pub struct DecodedAsset {
    /// Mono or stereo samples at the SOURCE rate (interleaved i16).
    pub samples: Vec<i16>,
    /// Source sample rate (e.g., 44100, 48000).
    pub sample_rate: u32,
    /// 1 = mono, 2 = stereo.
    pub channels: u16,
    /// Total sample frames (not interleaved scalars).
    pub frame_count: u64,
    /// Content hash for cache keying.
    pub content_hash: ContentHash,
}

impl DecodedAsset {
    /// Total interleaved samples (frame_count * channels).
    pub fn total_samples(&self) -> usize {
        self.frame_count as usize * self.channels as usize
    }

    /// Duration in seconds at source rate.
    pub fn duration_secs(&self) -> f64 {
        self.frame_count as f64 / self.sample_rate as f64
    }
}

/// Decode an asset to PCM using symphonia (spec Section17.3).
pub fn decode_asset(
    assets: &AssetResolver,
    asset_id: anim_core::id::AssetId,
) -> AudioResult<DecodedAsset> {
    let resolved = assets
        .resolve_verified(asset_id)
        .map_err(|_| AudioError::MissingAsset(asset_id.to_string()))?;

    // Read the asset file
    let path = &resolved.canonical_path;
    let mut file = File::open(path)
        .map_err(|e| AudioError::DecodeFailure(format!("open file: {}", e)))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| AudioError::DecodeFailure(format!("read file: {}", e)))?;

    let cursor = Cursor::new(bytes);
    let mss = MediaSourceStream::new(Box::new(cursor), Default::default());

    let mut hint = Hint::new();
    hint.with_extension("wav");
    hint.with_extension("flac");
    hint.with_extension("ogg");
    hint.with_extension("mp3");

    let format_opts = FormatOptions::default();
    let metadata_opts = MetadataOptions::default();
    let _decoder_opts = DecoderOptions::default();

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &format_opts, &metadata_opts)
        .map_err(|e| AudioError::DecodeFailure(e.to_string()))?;

    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| AudioError::DecodeFailure("no default track".into()))?;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| AudioError::DecodeFailure(e.to_string()))?;

    let mut all_samples = Vec::new();
    let mut total_frames = 0u64;
    let sample_rate = track.codec_params.sample_rate.unwrap_or(48000);
    let channels = track.codec_params.channels.map(|c| c.count() as u16).unwrap_or(2);

    // Validate channel count (mono or stereo only).
    if channels != 1 && channels != 2 {
        return Err(AudioError::UnsupportedChannelLayout(channels));
    }

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(e) => return Err(AudioError::DecodeFailure(e.to_string())),
        };

        let decoded = decoder
            .decode(&packet)
            .map_err(|e| AudioError::DecodeFailure(e.to_string()))?;

        match decoded {
            AudioBufferRef::S16(buf) => {
                let frames = buf.frames() as u64;
                total_frames += frames;
                let chans = buf.spec().channels.count();
                for frame in 0..frames as usize {
                    for ch in 0..chans {
                        all_samples.push(buf.chan(ch)[frame]);
                    }
                }
            }
            AudioBufferRef::F32(buf) => {
                let frames = buf.frames() as u64;
                total_frames += frames;
                let chans = buf.spec().channels.count();
                for frame in 0..frames as usize {
                    for ch in 0..chans {
                        let sample =
                            (buf.chan(ch)[frame].clamp(-1.0, 1.0) * 32767.0) as i16;
                        all_samples.push(sample);
                    }
                }
            }
            _ => return Err(AudioError::DecodeFailure("unsupported sample format".into())),
        }
    }

    let content_hash = resolved.entry.content_hash;
    Ok(DecodedAsset {
        samples: all_samples,
        sample_rate,
        channels,
        frame_count: total_frames,
        content_hash,
    })
}

/// Cache key for decoded assets (spec Section17.6: decoder_version + resampler_version + settings_hash).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DecodeCacheKey {
    pub content_hash: ContentHash,
    pub decoder_version: u32,
    pub resampler_version: u32,
    pub settings_hash: u64, // hash of OfflineAudioSpec + channel conversion settings
}

impl DecodeCacheKey {
    pub const DECODER_VERSION: u32 = 1;
    pub const RESAMPLER_VERSION: u32 = 1;
}
