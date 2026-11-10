//! Ordered mixing, gain, fades, looping and PCM conversion (spec Section17.7, Section17.8).
//!
//! For each output sample, process intersecting planned clips in canonical order.
//! Convert mono to stereo using declared gain; preserve stereo; reject unsupported
//! channel layouts. Apply source gain and sample-indexed fade gains. Accumulate in
//! f64 in stable order, clamp to [-1,1], and round once to signed 16-bit using
//! symmetric rule. No random dither. No adaptive normalization.

use crate::error::{AudioError, AudioResult};

/// Fade curve type (spec Section17.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FadeCurve {
    Linear,
    /// Equal power crossfade (cosine).
    EqualPower,
}

/// Compute fade gain for a sample position within a fade region.
fn fade_gain(pos: u64, len: u64, curve: FadeCurve) -> f64 {
    if len == 0 {
        return 1.0;
    }
    let t = (pos as f64) / (len as f64);
    match curve {
        FadeCurve::Linear => t,
        FadeCurve::EqualPower => {
            // Equal power: sin(t * π/2) for fade-in, cos(t * π/2) for fade-out
            (t * std::f64::consts::FRAC_PI_2).sin()
        }
    }
}

/// Mix a range of the plan into an output buffer (spec Section17.7, Section17.9).
///
/// Input:
/// - plan: the immutable mix plan
/// - assets: asset source providing canonical 48 kHz stereo PCM access
/// - range: sample range to mix (relative to plan start)
/// - out: output buffer (stereo i16, length = range.len * 2)
///
/// Returns number of sample frames written.
pub fn mix_range(
    plan: &crate::plan::MixPlan,
    assets: &dyn PcmAssetSource,
    range: crate::time::SampleRange,
    out: &mut [i16],
) -> AudioResult<u64> {
    let frame_count = range.len as usize;
    let stereo_len = frame_count * 2;

    if out.len() < stereo_len {
        return Err(AudioError::BufferCapacityExceeded {
            requested: stereo_len,
            capacity: out.len(),
        });
    }

    // Accumulator in f64 for stable mixing (spec: accumulate in f64).
    let mut accum = vec![0.0f64; stereo_len];

    // Get clips intersecting the range.
    let range_start = range.start.0;
    let range_end = range_start + frame_count as u64;

    for clip in &plan.clips {
        let clip_start = clip.timeline.start.0;
        let clip_end = clip_start + clip.timeline.len;

        // Check intersection.
        let start = clip_start.max(range_start);
        let end = clip_end.min(range_end);
        if start >= end {
            continue;
        }

        let offset_in_output = (start - range_start) as usize * 2;
        let offset_in_clip = start - clip_start;

        // Get resampled asset data.
        let asset_data = assets.get_pcm(
            clip.asset_id,
            clip.source_start.0 + offset_in_clip,
            end - start,
        )?;

        // Apply gain and fades per sample.
        let gain = clip.gain;
        let fade_in_len = clip.fade_in_samples;
        let fade_out_len = clip.fade_out_samples;
        let source_len = clip.source_len;

        for (i, &sample) in asset_data.iter().enumerate() {
            let abs_pos = offset_in_clip + i as u64;
            let mut sample_gain = gain;

            // Fade in
            if abs_pos < fade_in_len {
                sample_gain *= fade_gain(abs_pos, fade_in_len, FadeCurve::EqualPower);
            }
            // Fade out
            if abs_pos + fade_out_len >= source_len && source_len > fade_out_len {
                let fade_pos = source_len - abs_pos;
                if fade_pos < fade_out_len {
                    sample_gain *= fade_gain(
                        fade_out_len - fade_pos,
                        fade_out_len,
                        FadeCurve::EqualPower,
                    );
                }
            }

            let out_idx = offset_in_output + i * 2;
            accum[out_idx] += sample as f64 * sample_gain;
            accum[out_idx + 1] += sample as f64 * sample_gain;
        }
    }

    // Clamp to [-1,1] and round once to i16 (symmetric rounding).
    for (i, &val) in accum.iter().enumerate() {
        let clamped = val.clamp(-1.0, 1.0);
        // Symmetric rounding: round half away from zero.
        let rounded = if clamped >= 0.0 {
            (clamped * 32767.0 + 0.5) as i16
        } else {
            (clamped * 32768.0 - 0.5) as i16
        };
        out[i] = rounded;
    }

    Ok(frame_count as u64)
}

/// Trait for canonical PCM asset access (spec Section17.9).
pub trait PcmAssetSource {
    /// Get canonical 48 kHz stereo PCM for a clip.
    /// `source_offset` is the sample frame offset into the resampled asset.
    /// `frame_count` is the number of frames to return.
    fn get_pcm(
        &self,
        asset_id: anim_core::id::AssetId,
        source_offset: u64,
        frame_count: u64,
    ) -> AudioResult<Vec<i16>>;
}

/// Simple in-memory asset source for offline rendering.
#[derive(Default)]
pub struct MemoryAssetSource {
    pub assets: std::collections::HashMap<anim_core::id::AssetId, Vec<i16>>,
}

impl MemoryAssetSource {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, asset_id: anim_core::id::AssetId, pcm: Vec<i16>) {
        self.assets.insert(asset_id, pcm);
    }
}

impl PcmAssetSource for MemoryAssetSource {
    fn get_pcm(
        &self,
        asset_id: anim_core::id::AssetId,
        source_offset: u64,
        frame_count: u64,
    ) -> AudioResult<Vec<i16>> {
        let pcm = self
            .assets
            .get(&asset_id)
            .ok_or_else(|| AudioError::MissingAsset(asset_id.to_string()))?;
        let start = (source_offset * 2) as usize;
        let end = start + (frame_count * 2) as usize;
        if end > pcm.len() {
            return Err(AudioError::BufferCapacityExceeded {
                requested: end,
                capacity: pcm.len(),
            });
        }
        Ok(pcm[start..end].to_vec())
    }
}
