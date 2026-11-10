//! Render targets: formats, meanings and lifecycle (spec §15.6, §15.8).
//!
//! - `scene_hdr`  Rgba16Float  linear scene-referred HDR (MRT color)
//! - `depth`      Depth32Float clear 1.0, compare Less
//! - `normal`     Rgba16Float  view-space unit normal (xyz) + validity (w)
//! - `velocity`   Rgba16Float  current_uv - previous_uv + validity
//! - `post_hdr`   TWO Rgba16Float ping-pong targets
//! - output       surface/export-compatible, owned by the caller's
//!   configuration (anim_app surface / anim_export profile)
//!
//! Targets are recreated ATOMICALLY on extent or format change; texture
//! views are cached by target generation; stale views stay alive until
//! queued GPU work no longer references them (enforced in the GPU half
//! by keeping the old set until the submission fence passes).

use crate::error::{RenderError, RenderResult};

/// Pinned formats (§15.6). Missing support fails device creation
/// (`MissingRequiredFormat`) — never a silent reformat.
pub mod format {
    pub const SCENE_HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
    pub const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
    pub const NORMAL: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
    pub const VELOCITY: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
    pub const POST_HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
    /// Depth buffer clears to 1.0 (far); comparison is Less.
    pub const DEPTH_CLEAR: f32 = 1.0;
    pub const DEPTH_COMPARE: wgpu::CompareFunction = wgpu::CompareFunction::Less;

    /// All formats this crate requires from the adapter (§15.11: fail
    /// init for missing required formats/usages).
    pub const REQUIRED: &[wgpu::TextureFormat] =
        &[SCENE_HDR, DEPTH, NORMAL, VELOCITY, POST_HDR];
}

/// §15.8 RenderTargets. Textures are recreated together; `generation`
/// keys view caching and history bookkeeping.
pub struct RenderTargets {
    pub extent: wgpu::Extent3d,
    pub scene_hdr: wgpu::Texture,
    pub depth: wgpu::Texture,
    pub normal: wgpu::Texture,
    pub velocity: wgpu::Texture,
    pub post_hdr: [wgpu::Texture; 2],
    pub generation: u64,
}

/// Reject zero extents BEFORE touching the existing target set.
/// (Called by `resize` in the GPU half: a bad resize must leave the
/// last valid targets fully intact — §15.11.)
pub fn validate_extent(extent: wgpu::Extent3d) -> RenderResult<()> {
    if extent.width == 0 || extent.height == 0 || extent.depth_or_array_layers == 0 {
        return Err(RenderError::ZeroExtent(
            extent.width,
            extent.height,
            extent.depth_or_array_layers,
        ));
    }
    Ok(())
}

impl RenderTargets {
    /// Allocate a complete, consistent target set for `extent`.
    /// Wired into `Renderer::resize` in the GPU half (this chunk).
    pub(crate) fn create(
        device: &wgpu::Device,
        extent: wgpu::Extent3d,
    ) -> RenderResult<Self> {
        validate_extent(extent)?;
        // The post chain pings pong by SAMPLING previous outputs and
        // also runs one base copy back into scene_hdr → COPY_SRC |
        // COPY_DST are required beside attachment/sampling usage.
        let hdr_usage = wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST;
        let make_hdr = |label: &'static str| -> wgpu::Texture {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: format::SCENE_HDR,
                usage: hdr_usage,
                view_formats: &[],
            })
        };
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("anim_render.depth"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: format::DEPTH,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        Ok(Self {
            extent,
            scene_hdr: make_hdr("anim_render.scene_hdr"),
            depth,
            normal: make_hdr("anim_render.normal"),
            velocity: make_hdr("anim_render.velocity"),
            post_hdr: [
                make_hdr("anim_render.post_hdr[0]"),
                make_hdr("anim_render.post_hdr[1]"),
            ],
            generation: 0,
        })
    }
}

/// Validate an extent without constructing anything (unit-tested).
pub fn extent_is_valid(extent: wgpu::Extent3d) -> bool {
    validate_extent(extent).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_extents_are_rejected() {
        assert!(!extent_is_valid(wgpu::Extent3d {
            width: 0,
            height: 100,
            depth_or_array_layers: 1
        }));
        assert!(!extent_is_valid(wgpu::Extent3d {
            width: 100,
            height: 0,
            depth_or_array_layers: 1
        }));
        assert!(!extent_is_valid(wgpu::Extent3d {
            width: 100,
            height: 100,
            depth_or_array_layers: 0
        }));
        assert!(extent_is_valid(wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1
        }));
    }

    #[test]
    fn required_formats_cover_every_target() {
        assert_eq!(
            format::REQUIRED,
            &[
                wgpu::TextureFormat::Rgba16Float,
                wgpu::TextureFormat::Depth32Float,
                wgpu::TextureFormat::Rgba16Float,
                wgpu::TextureFormat::Rgba16Float,
                wgpu::TextureFormat::Rgba16Float,
            ]
        );
        assert_eq!(format::DEPTH_COMPARE, wgpu::CompareFunction::Less);
        assert_eq!(format::DEPTH_CLEAR, 1.0);
    }
}
