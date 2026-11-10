//! Per-draw instance data (Pod) for the scene/ordered-2D/transparent
//! passes plus the shared quad vertex buffer.
//!
//! One storage array per frame (§15.8: "Draw lists are rebuilt each
//! frame from immutable state"). Layout matches the WGSL `Instance*`
//! structs exactly: two column-major mat4x4, two vec4 (16-byte aligned
//! throughout). Alpha kinds are the shader dispatch table shared with
//! the WGSL files — constant names pinned in one place.

use crate::alpha::AlphaMode;
use crate::draw_list::DrawItem;
use bytemuck::{Pod, Zeroable};
use glam::Mat4;

/// Shader-side `flags.x` values (pinned: change = WGSL + Rust rename).
pub mod alpha_kind {
    /// Alpha-test path (cutoff in flags.y) for the opaque MRT pass.
    pub const ALPHA_TEST: f32 = 0.0;
    /// Straight-authored; shader decodes rgb · a.
    pub const STRAIGHT: f32 = 1.0;
    /// Premultiplied passthrough.
    pub const PREMULTIPLIED: f32 = 2.0;
    /// Additive premultiplied passthrough (pipeline blend One/One).
    pub const ADDITIVE: f32 = 3.0;
}

/// WGSL `Instance` twin (160 bytes, 16-byte aligned).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InstanceData {
    /// Column-major current world transform (glam's native layout).
    pub current: [[f32; 4]; 4],
    /// Column-major previous world transform (velocity pass).
    pub previous: [[f32; 4]; 4],
    /// Straight-authored linear tint.
    pub tint: [f32; 4],
    /// x: alpha_kind, y: cutoff, z: velocity_valid, w: pad.
    pub flags: [f32; 4],
}

unsafe impl Pod for InstanceData {}
unsafe impl Zeroable for InstanceData {}

impl InstanceData {
    pub fn from_item(item: &DrawItem) -> Self {
        let alpha_kind = match item.alpha_mode {
            AlphaMode::AlphaTest { cutoff } => {
                let _ = cutoff;
                alpha_kind::ALPHA_TEST
            }
            AlphaMode::Opaque => alpha_kind::ALPHA_TEST,
            AlphaMode::Straight => alpha_kind::STRAIGHT,
            AlphaMode::Premultiplied => alpha_kind::PREMULTIPLIED,
            AlphaMode::AdditivePremultiplied => alpha_kind::ADDITIVE,
        };
        let cutoff = match item.alpha_mode {
            AlphaMode::AlphaTest { cutoff } => cutoff,
            _ => 0.0,
        };
        Self {
            current: to_cols(item.current_transform),
            previous: to_cols(item.previous_transform),
            tint: item.tint,
            flags: [alpha_kind, cutoff, if item.velocity_valid { 1.0 } else { 0.0 }, 0.0],
        }
    }
}

/// glam column-major Mat4 as the bytemuck-friendly nested array.
pub fn to_cols(m: Mat4) -> [[f32; 4]; 4] {
    // glam's `to_cols_array_2d` returns the array of COLUMNS, matching from_cols_array_2d.
    m.to_cols_array_2d()
}

/// Quad corners in triangle-strip order (matches every WGSL CORNERS).
pub const QUAD_VERTICES: &[[f32; 4]] = &[
    [-1.0, -1.0, 0.0, 1.0],
    [1.0, -1.0, 0.0, 1.0],
    [-1.0, 1.0, 0.0, 1.0],
    [1.0, 1.0, 0.0, 1.0],
];

/// How many vertices one instance occupies.
pub const QUAD_VERTEX_COUNT: u32 = 4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_layout_matches_wgsl_commit() {
        // 2 × mat4x4 + 2 × vec4 = 64 + 64 + 16 + 16 bytes, 4-aligned,
        // matching the WGSL `Instance` struct exactly.
        assert_eq!(std::mem::size_of::<InstanceData>(), 160);
        assert_eq!(std::mem::align_of::<InstanceData>(), 4);
        let zero = <InstanceData as Zeroable>::zeroed();
        assert_eq!(zero.flags, [0.0; 4]);
        let m = glam::Mat4::IDENTITY;
        assert_eq!(to_cols(m), m.to_cols_array_2d());
    }
}
