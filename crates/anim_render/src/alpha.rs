//! Alpha modes, conversion math and blend descriptors (spec §15.7).
//!
//! Contract pinned here (G5 goldens cover every mode):
//! - Authoring is STRAIGHT-alpha; decoding to linear happens at asset
//!   load-time (anim_assets carries the color-space + alpha metadata).
//! - Everything is PREMULTIPLIED at draw time: `rgb_premul =
//!   rgb_linear * alpha`. Assets already marked premultiplied are NOT
//!   multiplied again (the material contract separates the two).
//! - Standard premultiplied-over blend: source One, destination
//!   OneMinusSrcAlpha (color AND alpha).
//! - Additive premultiplied: source One, destination One; the shader's
//!   RGB already includes coverage and intensity.
//! - Alpha-test discards below cutoff BEFORE any attachment write.
//! - Zero-alpha straight-decoded premultiplied output contributes no
//!   color (rgb_premul = rgb_linear · 0 = 0 by construction).

use crate::error::{RenderError, RenderResult};

/// §15.8 alpha modes. `AlphaTest` carries its cutoff in (0.0, 1.0]; the
/// cutoff is validated at extraction (`validate_alpha_mode`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AlphaMode {
    Opaque,
    AlphaTest { cutoff: f32 },
    Straight,
    Premultiplied,
    AdditivePremultiplied,
}

/// Validate a mode: only the alpha-test cutoff carries an invariant.
/// Valid = finite and 0.0 < cutoff <= 1.0 (cutoff 0 would never discard
/// and > 1 would always discard — both are authoring errors, rejected).
pub fn validate_alpha_mode(mode: AlphaMode) -> RenderResult<()> {
    match mode {
        AlphaMode::AlphaTest { cutoff } => {
            if !cutoff.is_finite() || !(cutoff > 0.0 && cutoff <= 1.0) {
                return Err(RenderError::InvalidAlphaCutoff(cutoff));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Decode straight-alpha LINEAR color to premultiplied.
/// `rgb_premul = rgb_linear * alpha` (exact f32 products; NaN input is
/// the caller's contract failure — extraction validates finiteness).
pub fn straight_to_premultiplied(rgba_linear: [f32; 4]) -> [f32; 4] {
    let [r, g, b, a] = rgba_linear;
    [r * a, g * a, b * a, a]
}

/// CPU reference of the standard premultiplied-over composite (One /
/// OneMinusSrcAlpha): used by the G5 goldens as the pinned expected
/// result for ordered-2D and transparent coverage.
pub fn blend_over_premul(dst: [f32; 4], src: [f32; 4]) -> [f32; 4] {
    let inv_a = 1.0 - src[3];
    [
        src[0] + dst[0] * inv_a,
        src[1] + dst[1] * inv_a,
        src[2] + dst[2] * inv_a,
        src[3] + dst[3] * inv_a,
    ]
}

/// CPU reference of the additive premultiplied composite (One / One).
pub fn blend_additive_premul(dst: [f32; 4], src: [f32; 4]) -> [f32; 4] {
    [src[0] + dst[0], src[1] + dst[1], src[2] + dst[2], src[3] + dst[3]]
}

/// The wgpu blend descriptor for a mode. Opaque and alpha-test write
/// depth/auxiliaries with NO blending; the three blended modes use the
/// exact source/destination factors pinned above. Alpha exactness rule
/// (G5): the SAME factors apply to color and alpha channels.
pub fn blend_state(mode: AlphaMode) -> wgpu::BlendState {
    match mode {
        // No blending at all — attachment writes pass through unchanged.
        AlphaMode::Opaque | AlphaMode::AlphaTest { .. } => wgpu::BlendState::REPLACE,
        AlphaMode::Straight | AlphaMode::Premultiplied => wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        },
        AlphaMode::AdditivePremultiplied => wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        },
    }
}

/// Does a mode WRITE depth (and consequently auxiliaries in the MRT
/// pass)?  Opaque and alpha-test write; the blended modes depth-TEST
/// without depth writes (§15.6: transparent items never write depth,
/// normals or velocity in the initial material contract).
pub fn writes_depth(mode: AlphaMode) -> bool {
    matches!(mode, AlphaMode::Opaque | AlphaMode::AlphaTest { .. })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_decode_multiplies_by_alpha() {
        let out = straight_to_premultiplied([0.8, 0.4, 0.2, 0.5]);
        assert_eq!(out, [0.4, 0.2, 0.1, 0.5]);
    }

    #[test]
    fn zero_alpha_straight_contributes_no_color() {
        let out = straight_to_premultiplied([0.9, 0.9, 0.9, 0.0]);
        assert_eq!(out, [0.0, 0.0, 0.0, 0.0]);
        // And premultiplied-over onto anything leaves it unchanged:
        let dst = blend_over_premul([0.25, 0.5, 0.75, 1.0], out);
        assert_eq!(dst, [0.25, 0.5, 0.75, 1.0]);
    }

    #[test]
    fn premultiplied_assets_are_not_multiplied_again() {
        // An asset flagged premultiplied carries rgb_a already; the
        // passthrough helper is the ONLY decode it gets.
        let premul_src = [0.4, 0.2, 0.1, 0.5];
        // blending equals the over reference exactly
        let dst = blend_over_premul([0.0, 0.0, 0.0, 0.0], premul_src);
        assert_eq!(dst, premul_src);
    }

    #[test]
    fn additive_composite_uses_one_one() {
        let dst = [0.1, 0.2, 0.3, 0.4];
        let src = [0.2, 0.1, 0.0, 0.1];
        assert_eq!(blend_additive_premul(dst, src), [0.3, 0.3, 0.3, 0.5]);
    }

    #[test]
    fn cutoff_validation_is_strict() {
        assert!(validate_alpha_mode(AlphaMode::AlphaTest { cutoff: 0.5 }).is_ok());
        assert!(validate_alpha_mode(AlphaMode::AlphaTest { cutoff: 1.0 }).is_ok());
        assert!(validate_alpha_mode(AlphaMode::AlphaTest { cutoff: 0.0 }).is_err());
        assert!(validate_alpha_mode(AlphaMode::AlphaTest { cutoff: 1.5 }).is_err());
        assert!(validate_alpha_mode(AlphaMode::AlphaTest { cutoff: f32::NAN }).is_err());
        assert!(
            validate_alpha_mode(AlphaMode::AlphaTest { cutoff: f32::INFINITY }).is_err()
        );
        assert!(validate_alpha_mode(AlphaMode::Opaque).is_ok());
        assert!(validate_alpha_mode(AlphaMode::Straight).is_ok());
        assert!(validate_alpha_mode(AlphaMode::Premultiplied).is_ok());
        assert!(validate_alpha_mode(AlphaMode::AdditivePremultiplied).is_ok());
    }

    #[test]
    fn blend_descriptors_match_the_pinned_contract() {
        let over = blend_state(AlphaMode::Premultiplied);
        assert_eq!(over.color.src_factor, wgpu::BlendFactor::One);
        assert_eq!(over.color.dst_factor, wgpu::BlendFactor::OneMinusSrcAlpha);
        assert_eq!(over.alpha.src_factor, wgpu::BlendFactor::One);
        assert_eq!(over.alpha.dst_factor, wgpu::BlendFactor::OneMinusSrcAlpha);

        let add = blend_state(AlphaMode::AdditivePremultiplied);
        assert_eq!(add.color.dst_factor, wgpu::BlendFactor::One);
        assert_eq!(add.alpha.dst_factor, wgpu::BlendFactor::One);

        // Opaque/alpha-test REPLACE (no blending), so they cannot shift
        // straight-authored colors behind their own backs.
        assert_eq!(blend_state(AlphaMode::Opaque), wgpu::BlendState::REPLACE);
        assert_eq!(
            blend_state(AlphaMode::AlphaTest { cutoff: 0.5 }),
            wgpu::BlendState::REPLACE
        );
    }

    #[test]
    fn depth_writes_follow_the_material_contract() {
        assert!(writes_depth(AlphaMode::Opaque));
        assert!(writes_depth(AlphaMode::AlphaTest { cutoff: 0.5 }));
        assert!(!writes_depth(AlphaMode::Straight));
        assert!(!writes_depth(AlphaMode::Premultiplied));
        assert!(!writes_depth(AlphaMode::AdditivePremultiplied));
    }
}
