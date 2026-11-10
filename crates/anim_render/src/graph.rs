//! Concrete pass sequence and resource declarations (spec §15.5, §15.6).
//!
//! The frame pass order is VERSIONED and FIXED:
//!   1. opaque MRT       (HDR + depth + normals + velocity)
//!   2. ordered 2D       (project layer order; unlit 2D composition)
//!   3. transparent      (depth-TEST only, no depth/aux writes)
//!   4. post chain       (linear HDR: depth-aware, motion blur, bloom)
//!   5. tone map         (EXACTLY ONCE)
//!   6. grade            (display-referred)
//!   7. output transfer  (declared profile; export vs surface)
//!
//! Editor UI (egui/overlays/gizmos) composites AFTER the output target
//! of the clean scene — it is a SEPARATE operation (§15.9
//! `composite_editor_ui`) and never enters export.
//!
//! DISABLED-pass semantics (§15.7 Post graph): a disabled pass declares
//! no reads/writes and returns its input target. Ping-pong targets may
//! alias ONLY when graph lifetimes do not overlap (checked in the GPU
//! half when wiring the chain; the declarations here are the pinned
//! order the wiring must follow).

use crate::error::{RenderError, RenderResult};

/// Material/pass selectors for `DrawItem.material_pass`.
pub const MATERIAL_OPAQUE_MRT: u16 = 0;
pub const MATERIAL_ORDERED_2D: u16 = 1;
pub const MATERIAL_TRANSPARENT: u16 = 2;

/// Draw-layer band for simulation particles: ABOVE all authored scene
/// content (z quantizes at ±2^29 max; this band is above it), below any
/// editor/UI composition. Documented in ADR-0010.
pub const PARTICLE_LAYER_ORDER: i32 = i32::MAX / 4;

/// The frame's main passes in execution order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainPass {
    OpaqueMrt,
    Ordered2D,
    Transparent,
    PostChain,
    OutputTransfer,
}

/// PINNED execution order. Any change is a graph VERSION bump (G5
/// command-log verification replays exactly this log).
pub const MAIN_PASS_SEQUENCE: &[MainPass] = &[
    MainPass::OpaqueMrt,
    MainPass::Ordered2D,
    MainPass::Transparent,
    MainPass::PostChain,
    MainPass::OutputTransfer,
];

/// Post-chain passes. `ToneMap` is MANDATORY and runs EXACTLY ONCE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostPass {
    DepthAware,
    MotionBlur,
    Bloom,
    ToneMap,
    Grade,
    OutputTransfer,
}

/// Versioned post order (§15.7): depth-aware effects, motion blur,
/// bloom and other linear HDR effects; tone map; display-referred
/// grade; output transfer.
pub const POST_PASS_SEQUENCE: &[PostPass] = &[
    PostPass::DepthAware,
    PostPass::MotionBlur,
    PostPass::Bloom,
    PostPass::ToneMap,
    PostPass::Grade,
    PostPass::OutputTransfer,
];

/// Declared effect settings the post chain consumes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PostSettings {
    pub depth_aware_enabled: bool,
    pub motion_blur_enabled: bool,
    pub bloom_enabled: bool,
    pub grade_enabled: bool,
    /// MUST be true — tone mapping runs exactly once; `false` is a
    /// malformed configuration, not a feature.
    pub tone_map_enabled: bool,
}

impl Default for PostSettings {
    fn default() -> Self {
        Self {
            depth_aware_enabled: false,
            motion_blur_enabled: true,
            bloom_enabled: true,
            grade_enabled: true,
            tone_map_enabled: true,
        }
    }
}

/// Enable-filtered post chain in the pinned versioned order. A disabled
/// pass declares no reads/writes and returns its input target — which
/// for planning purposes means it simply does not appear in the
/// executed chain (its neighbors read/write the same target).
pub fn post_chain(settings: &PostSettings) -> RenderResult<Vec<PostPass>> {
    if !settings.tone_map_enabled {
        return Err(RenderError::InvalidGraph(
            "tone map must run exactly once; disabling it is a malformed configuration"
                .into(),
        ));
    }
    Ok(POST_PASS_SEQUENCE
        .iter()
        .copied()
        .filter(|p| match p {
            PostPass::DepthAware => settings.depth_aware_enabled,
            PostPass::MotionBlur => settings.motion_blur_enabled,
            PostPass::Bloom => settings.bloom_enabled,
            PostPass::Grade => settings.grade_enabled,
            PostPass::ToneMap | PostPass::OutputTransfer => true,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_sequence_is_pinned_and_ordered() {
        assert_eq!(
            MAIN_PASS_SEQUENCE,
            &[
                MainPass::OpaqueMrt,
                MainPass::Ordered2D,
                MainPass::Transparent,
                MainPass::PostChain,
                MainPass::OutputTransfer,
            ]
        );
        assert_eq!(MATERIAL_OPAQUE_MRT, 0);
        assert_eq!(MATERIAL_ORDERED_2D, 1);
        assert_eq!(MATERIAL_TRANSPARENT, 2);
    }

    #[test]
    fn post_chain_honors_enable_flags_in_versioned_order() {
        let all = post_chain(&PostSettings {
            depth_aware_enabled: true,
            ..PostSettings::default()
        })
        .unwrap();
        assert_eq!(
            all,
            vec![
                PostPass::DepthAware,
                PostPass::MotionBlur,
                PostPass::Bloom,
                PostPass::ToneMap,
                PostPass::Grade,
                PostPass::OutputTransfer,
            ]
        );

        let minimal = post_chain(&PostSettings {
            depth_aware_enabled: false,
            motion_blur_enabled: false,
            bloom_enabled: false,
            grade_enabled: false,
            tone_map_enabled: true,
        })
        .unwrap();
        // ToneMap + OutputTransfer remain: tone map ONCE, transfer once.
        assert_eq!(minimal, vec![PostPass::ToneMap, PostPass::OutputTransfer]);
    }

    #[test]
    fn tone_map_can_never_be_disabled() {
        assert!(post_chain(&PostSettings {
            tone_map_enabled: false,
            ..PostSettings::default()
        })
        .is_err());
        assert!(post_chain(&PostSettings {
            tone_map_enabled: false,
            depth_aware_enabled: false,
            motion_blur_enabled: false,
            bloom_enabled: false,
            grade_enabled: false,
        })
        .is_err());
    }

    #[test]
    fn tone_map_appears_exactly_once() {
        let chain = post_chain(&PostSettings::default()).unwrap();
        assert_eq!(chain.iter().filter(|p| **p == PostPass::ToneMap).count(), 1);
        assert_eq!(chain.iter().filter(|p| **p == PostPass::OutputTransfer).count(), 1);
    }
}
