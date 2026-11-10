//! Locked rubato configuration and canonical 48 kHz conversion (spec Section17.3, Section17.6, Section17.7).
//!
//! Resampler implementation, coefficients/settings, channel conversion, mix
//! order and output rounding are versioned. Streaming resampler state is
//! private to an asset decode session and independent of output mix chunk size.
//! Source position for an output sample: source_start + (output_sample - clip_timeline_start).
//! Looping uses checked modulo by canonical source length.

// Clippy allowances for explicit indexing in DSP loops.
#![allow(clippy::needless_range_loop)]

use crate::decode::{DecodeCacheKey, DecodedAsset};
use crate::error::{AudioError, AudioResult};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

/// Locked rubato sinc configuration (spec Section17.3, Section17.7).
/// These parameters are FIXED for determinism across all renders.
const RESAMPLER_PARAMS: SincInterpolationParameters = SincInterpolationParameters {
    sinc_len: 256,
    f_cutoff: 0.95,
    interpolation: SincInterpolationType::Linear,
    oversampling_factor: 256,
    window: WindowFunction::BlackmanHarris2,
};

// Canonical output format (Section 17.8).
pub const OUTPUT_SAMPLE_RATE: u32 = 48_000;
pub const OUTPUT_CHANNELS: u16 = 2;

/// Create a new resampler for a channel.
fn create_resampler(ratio: f64) -> AudioResult<SincFixedIn<f64>> {
    let params = RESAMPLER_PARAMS;
    let chunk_size = 1024;
    SincFixedIn::<f64>::new(ratio, 2.0, params, chunk_size, 1)
        .map_err(|e| AudioError::ResampleFailure(e.to_string()))
}

/// Resample an asset to canonical 48 kHz stereo (spec Section17.7).
///
/// Input: decoded asset at source rate/channels.
/// Output: mono or stereo i16 samples at 48 kHz.
pub fn resample_to_canonical(
    asset: &DecodedAsset,
    _cache_key: &DecodeCacheKey,
) -> AudioResult<Vec<i16>> {
    // If already at canonical rate and channels, just convert if needed.
    if asset.sample_rate == OUTPUT_SAMPLE_RATE && asset.channels == OUTPUT_CHANNELS {
        return Ok(asset.samples.clone());
    }

    // Create resampler for each channel.
    let ratio = OUTPUT_SAMPLE_RATE as f64 / asset.sample_rate as f64;
    let _params = RESAMPLER_PARAMS;
    let chunk_size = 1024; // processing chunk size

    let mut resamplers = Vec::new();
    for _ in 0..asset.channels as usize {
        let resampler = create_resampler(ratio)?;
        resamplers.push(resampler);
    }

    // De-interleave input by channel.
    let frame_count = asset.frame_count as usize;
    let mut input_channels = vec![Vec::with_capacity(frame_count); asset.channels as usize];
    for frame in 0..frame_count {
        for ch in 0..asset.channels as usize {
            input_channels[ch]
                .push(asset.samples[frame * asset.channels as usize + ch] as f64 / 32768.0);
        }
    }

    // Resample each channel.
    let mut output_channels = Vec::new();
    for (ch, mut resampler) in resamplers.into_iter().enumerate() {
        let input = &input_channels[ch];
        let mut output = Vec::new();
        let mut input_pos = 0;

        while input_pos < input.len() {
            let chunk_end = (input_pos + chunk_size).min(input.len());
            let chunk = &input[input_pos..chunk_end];
            let resampled = resampler
                .process(&[chunk], None)
                .map_err(|e| AudioError::ResampleFailure(e.to_string()))?;
            output.extend_from_slice(&resampled[0]);
            input_pos = chunk_end;
        }
        output_channels.push(output);
    }

    // Interleave output channels (stereo output).
    let out_frames = output_channels[0].len();
    let mut output = Vec::with_capacity(out_frames * OUTPUT_CHANNELS as usize);

    if asset.channels == 1 {
        // Mono source → duplicate to stereo.
        for i in 0..out_frames {
            let sample = (output_channels[0][i].clamp(-1.0, 1.0) * 32767.0) as i16;
            output.push(sample); // left
            output.push(sample); // right
        }
    } else {
        // Stereo source → keep stereo (or if output is mono, would mix down).
        for i in 0..out_frames {
            output.push((output_channels[0][i].clamp(-1.0, 1.0) * 32767.0) as i16);
            output.push((output_channels[1][i].clamp(-1.0, 1.0) * 32767.0) as i16);
        }
    }

    Ok(output)
}

/// Streaming resampler for chunked output (spec Section17.7: state private to decode session).
pub struct StreamingResampler {
    resamplers: Vec<SincFixedIn<f64>>,
    input_buffer: Vec<Vec<f64>>,
    channel_count: usize,
}

impl StreamingResampler {
    pub fn new(asset: &DecodedAsset) -> AudioResult<Self> {
        let ratio = OUTPUT_SAMPLE_RATE as f64 / asset.sample_rate as f64;
        let _params = RESAMPLER_PARAMS;
        let _chunk_size = 1024;

        let mut resamplers = Vec::new();
        for _ in 0..asset.channels as usize {
            let resampler = create_resampler(ratio)?;
            resamplers.push(resampler);
        }

        Ok(Self {
            resamplers,
            input_buffer: vec![Vec::new(); asset.channels as usize],
            channel_count: asset.channels as usize,
        })
    }

    /// Process a chunk of input frames (interleaved), return output frames (interleaved stereo).
    pub fn process_chunk(&mut self, input: &[i16]) -> AudioResult<Vec<i16>> {
        let frame_count = input.len() / self.channel_count;

        // De-interleave
        for ch in 0..self.channel_count {
            self.input_buffer[ch].clear();
            self.input_buffer[ch].reserve(frame_count);
            for frame in 0..frame_count {
                self.input_buffer[ch]
                    .push(input[frame * self.channel_count + ch] as f64 / 32768.0);
            }
        }

        // Resample each channel
        let mut output_channels = Vec::new();
        for ch in 0..self.channel_count {
            let resampled = self.resamplers[ch]
                .process(&[&self.input_buffer[ch]], None)
                .map_err(|e| AudioError::ResampleFailure(e.to_string()))?;
            output_channels.push(resampled[0].clone());
        }

        // Interleave to stereo output
        let out_frames = output_channels[0].len();
        let mut output = Vec::with_capacity(out_frames * OUTPUT_CHANNELS as usize);
        if self.channel_count == 1 {
            for i in 0..out_frames {
                let sample = (output_channels[0][i].clamp(-1.0, 1.0) * 32767.0) as i16;
                output.push(sample);
                output.push(sample);
            }
        } else {
            for i in 0..out_frames {
                output.push((output_channels[0][i].clamp(-1.0, 1.0) * 32767.0) as i16);
                output.push((output_channels[1][i].clamp(-1.0, 1.0) * 32767.0) as i16);
            }
        }
        Ok(output)
    }

    /// Flush any remaining samples.
    pub fn flush(&mut self) -> AudioResult<Vec<i16>> {
        let mut output = Vec::new();
        for ch in 0..self.channel_count {
            let resampled = self.resamplers[ch]
                .process(&[&[]], None)
                .map_err(|e| AudioError::ResampleFailure(e.to_string()))?;
            if ch == 0 {
                output.resize(resampled[0].len() * OUTPUT_CHANNELS as usize, 0);
            }
            for (i, &sample) in resampled[0].iter().enumerate() {
                let idx = i * OUTPUT_CHANNELS as usize + ch.min(1);
                output[idx] = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
            }
        }
        Ok(output)
    }
}
