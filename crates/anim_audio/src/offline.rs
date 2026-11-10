//! Chunk orchestration and deterministic offline rendering (spec Section17.3, Section17.5, Section17.9).
//!
//! Offline rendering is a pure function of the immutable project/scene snapshot,
//! verified assets and OfflineAudioSpec. Timeline scheduling uses integer samples;
//! floating-point seconds never determine clip placement (Section17.6).
//! Resampler implementation, coefficients/settings, channel conversion, mix order
//! and output rounding are versioned. No random dither is applied.

use crate::decode::decode_asset;
use crate::error::AudioResult;
use crate::mix::MemoryAssetSource;
use crate::plan::compile_plan;
use crate::resample::resample_to_canonical;
use crate::time::OfflineAudioSpec;
use crate::wav::write_wav;

/// Offline rendering context (stateless, deterministic).
pub struct OfflineRenderer {
    spec: OfflineAudioSpec,
}

impl OfflineRenderer {
    pub fn new(spec: OfflineAudioSpec) -> Self {
        Self { spec }
    }

    /// Full offline render: scene + assets -> WAV bytes (spec Section17.9).
    pub fn render<W: std::io::Write + std::io::Seek>(
        &self,
        scene: &anim_scene::SceneSnapshot,
        assets: &anim_assets::AssetResolver,
        writer: W,
    ) -> AudioResult<u64> {
        // 1. Compile the immutable mix plan (spec Section17.5 step 2).
        let plan = compile_plan(scene, assets, &self.spec)?;

        // 2. Decode and resample all required assets to canonical 48 kHz stereo.
        let mut asset_source = MemoryAssetSource::new();
        for clip in &plan.clips {
            if asset_source.assets.contains_key(&clip.asset_id) {
                continue; // already processed
            }
            let decoded = decode_asset(assets, clip.asset_id)?;
            let cache_key = crate::decode::DecodeCacheKey {
                content_hash: decoded.content_hash,
                decoder_version: crate::decode::DecodeCacheKey::DECODER_VERSION,
                resampler_version: crate::decode::DecodeCacheKey::RESAMPLER_VERSION,
                settings_hash: 0,
            };
            let resampled = resample_to_canonical(&decoded, &cache_key)?;
            asset_source.assets.insert(clip.asset_id, resampled);
        }

        // 3. Determine sample range to render.
        let range = self.spec.sample_range()?;

        // 4. Write WAV output (spec Section17.9).
        let written = write_wav(&plan, &asset_source, range, writer)?;

        Ok(written)
    }

    /// Render to a provided buffer (for testing/chunked output).
    pub fn render_to_buffer(
        &self,
        scene: &anim_scene::SceneSnapshot,
        assets: &anim_assets::AssetResolver,
    ) -> AudioResult<Vec<i16>> {
        let plan = compile_plan(scene, assets, &self.spec)?;
        let mut asset_source = MemoryAssetSource::new();

        for clip in &plan.clips {
            if asset_source.assets.contains_key(&clip.asset_id) {
                continue;
            }
            let decoded = decode_asset(assets, clip.asset_id)?;
            let cache_key = crate::decode::DecodeCacheKey {
                content_hash: decoded.content_hash,
                decoder_version: crate::decode::DecodeCacheKey::DECODER_VERSION,
                resampler_version: crate::decode::DecodeCacheKey::RESAMPLER_VERSION,
                settings_hash: 0,
            };
            let resampled = resample_to_canonical(&decoded, &cache_key)?;
            asset_source.assets.insert(clip.asset_id, resampled);
        }

        let range = self.spec.sample_range()?;
        let frame_count = range.len as usize;
        let mut output = vec![0i16; frame_count * 2];

        crate::mix::mix_range(&plan, &asset_source, range, &mut output)?;

        Ok(output)
    }
}

/// Offline rendering is a pure function - same inputs = same outputs.
/// This function is the primary offline entry point (spec Section17.9).
pub fn render_offline(
    scene: &anim_scene::SceneSnapshot,
    assets: &anim_assets::AssetResolver,
    spec: &OfflineAudioSpec,
    writer: impl std::io::Write + std::io::Seek,
) -> AudioResult<u64> {
    let renderer = OfflineRenderer::new(spec.clone());
    renderer.render(scene, assets, writer)
}
