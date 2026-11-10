//! Camera matrices (current/previous view-projection), cut state and the
//! render history model (spec §15.7 velocity, §15.8 HistoryState).
//!
//! ## Camera convention (pinned for WP06, recorded in ADR-0010)
//! - 2D orthographic camera looking along +z: larger world z = FARTHER
//!   from the viewer. A snapshot node's 2D affine transform carries no
//!   z, so scene-derived items sit at z=0 (mid-depth).
//! - World +y is UP; NDC is y-up (D3D/WGPU clip space); depth range is
//!   [0, 1] via the RH orthographic projection (near plane at
//!   `VIEW_DEPTH_NEAR`, far at `VIEW_DEPTH_FAR`).
//! - The projection's world height is the documented constant
//!   `VIEW_HEIGHT_WORLD`; width follows the TARGET aspect from
//!   `PreviousFrameState.viewport_aspect` (updated by the owner at
//!   resize — a change is a `ProjectionChange` history invalidation).
//! - No active camera in a snapshot is a DEFAULT identity camera
//!   (center 0, rotation 0, zoom 1) — documented, not a fallback.

use crate::error::{RenderError, RenderResult};
use anim_scene::snapshot::EvaluatedCamera;
use glam::Mat4;

/// World-space height the orthographic projection maps to NDC [-1, +1].
pub const VIEW_HEIGHT_WORLD: f64 = 10.0;

/// Orthographic depth range in world z units (camera looks along +z).
pub const VIEW_DEPTH_NEAR: f32 = -1_000.0;
pub const VIEW_DEPTH_FAR: f32 = 1_000.0;

/// View depth quantization used by the transparent sort (spec §15.7:
/// "quantize depth to signed i32 in documented camera units"). Units:
/// 1 quantum = 1/64 world unit, so [NEAR, FAR] spans ±64,000 quanta —
/// far inside i32 range with headroom for future 3D content.
pub const DEPTH_QUANTUM_SCALE: f32 = 64.0;

/// Quantize a view depth (world z, larger = farther) for SORTING only.
/// The stored `DrawItem.view_depth` stays f32; this is the comparator's
/// key. Ties after quantization are resolved by the remaining key
/// fields — never by hash-map or GPU timing order.
pub fn quantize_view_depth(depth: f32) -> RenderResult<i32> {
    if !depth.is_finite() {
        return Err(RenderError::NonfiniteViewDepth("non-finite depth".into()));
    }
    let scaled = depth * DEPTH_QUANTUM_SCALE;
    if scaled > i32::MAX as f32 || scaled < i32::MIN as f32 {
        return Err(RenderError::NonfiniteViewDepth("depth out of quantized range".into()));
    }
    Ok(scaled.round() as i32)
}

/// Current AND previous view-projection (they differ across a moving
/// camera or an interpolated tick; they are EQUAL on the first frame).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraMatrices {
    pub current_view_proj: Mat4,
    pub previous_view_proj: Mat4,
}

impl CameraMatrices {
    /// Build both matrices from the cut-resolved active camera (or None
    /// for the documented identity default) and the target aspect.
    pub fn eval2d(camera: Option<&EvaluatedCamera>, aspect: f64) -> RenderResult<Self> {
        if !aspect.is_finite() || aspect <= 0.0 {
            return Err(RenderError::NonfiniteCamera("viewport aspect".into()));
        }
        let (cx, cy, rot, zoom) = match camera {
            None => (0.0, 0.0, 0.0, 1.0),
            Some(c) => (c.center_x, c.center_y, c.rotation, c.zoom),
        };
        for (name, v) in
            [("center_x", cx), ("center_y", cy), ("rotation", rot), ("zoom", zoom)]
        {
            if !v.is_finite() {
                return Err(RenderError::NonfiniteCamera(name.into()));
            }
        }
        if zoom <= 0.0 {
            return Err(RenderError::NonfiniteCamera("zoom must be > 0".into()));
        }

        // Inverse view: translate to camera center, un-rotate, un-zoom
        // (zoom z shows VIEW_HEIGHT_WORLD*z world units tall).
        // f64 authored values are narrowed to the f32 render pipeline
        // here — the narrowing point is central and deterministic.
        let inv_view =
            Mat4::from_translation(glam::Vec3::new(-(cx as f32), -(cy as f32), 0.0))
                * Mat4::from_rotation_z(-(rot as f32))
                * Mat4::from_scale(glam::Vec3::splat(1.0 / zoom as f32));

        let half_h = (VIEW_HEIGHT_WORLD * 0.5) as f32;
        let half_w = half_h * (aspect as f32);
        let proj = Mat4::orthographic_rh(
            -half_w,
            half_w,
            -half_h,
            half_h,
            VIEW_DEPTH_NEAR,
            VIEW_DEPTH_FAR,
        );
        let vp = proj * inv_view;
        Ok(Self {
            current_view_proj: vp,
            previous_view_proj: vp, // caller (extract_frame) replaces with history when valid
        })
    }

    /// Project a world point to NDC with the CURRENT matrix
    /// (velocity math reference; the shader repeats it exactly).
    pub fn project_current(&self, world: glam::Vec3) -> glam::Vec3 {
        self.current_view_proj.project_point3(world)
    }
}

/// History validity lifecycle (spec §15.7): these events invalidate
/// motion-vector history. Nothing else does (and as of WP06 the list is
/// EXHAUSTIVE — no heuristic invalidation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryInvalidation {
    SceneLoad,
    DiscontinuousSeek,
    Teleport,
    CameraCut,
    ProjectionChange,
    TopologyDiscontinuity,
    TargetResize,
}

/// §15.8 HistoryState. `previous_tick` is `Option` (no history before
/// the first successful submission — the only tolerance beyond the spec
/// sketch, recorded in ADR-0010).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HistoryState {
    pub valid: bool,
    /// Target generation the previous frame rendered against.
    pub target_generation: u64,
    pub previous_view_proj: Mat4,
    pub previous_tick: Option<anim_sim::SimTick>,
    /// Monotone serial bumped on EVERY CameraCut; consumers diff it to
    /// detect cuts without comparing matrices.
    pub cut_serial: u64,
}

impl HistoryState {
    /// Fresh session state: NO valid history (first frame is invalid
    /// until the initial SceneLoad → successful submission).
    pub fn fresh() -> Self {
        Self {
            valid: false,
            target_generation: 0,
            previous_view_proj: Mat4::IDENTITY,
            previous_tick: None,
            cut_serial: 0,
        }
    }

    /// Apply an invalidation reason. Only CameraCut additionally bumps
    /// the cut serial (its identity marker).
    pub fn invalidate(&mut self, reason: &HistoryInvalidation) {
        self.valid = false;
        if *reason == HistoryInvalidation::CameraCut {
            self.cut_serial = self.cut_serial.wrapping_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera(cx: f64, cy: f64, rot: f64, zoom: f64) -> EvaluatedCamera {
        EvaluatedCamera {
            id: anim_core::NodeId::new_v4(),
            center_x: cx,
            center_y: cy,
            rotation: rot,
            zoom,
            active: true,
        }
    }

    #[test]
    fn default_identity_camera_centers_the_origin() {
        let cam = CameraMatrices::eval2d(None, 1.0).unwrap();
        let p = cam.project_current(glam::Vec3::ZERO);
        // center of view → origin NDC
        assert!(p.x.abs() < 1e-5 && p.y.abs() < 1e-5);
    }

    #[test]
    fn world_height_maps_to_full_ndc() {
        let cam = CameraMatrices::eval2d(None, 1.0).unwrap();
        let top = cam.project_current(glam::Vec3::new(
            0.0,
            (VIEW_HEIGHT_WORLD * 0.5) as f32,
            0.0,
        ));
        assert!(
            (top.y - 1.0).abs() < 1e-4,
            "top of view height should reach NDC +1, got {top:?}"
        );
        let right = cam.project_current(glam::Vec3::new(
            (VIEW_HEIGHT_WORLD * 0.5) as f32,
            0.0,
            0.0,
        ));
        assert!((right.x - 1.0).abs() < 1e-4, "square aspect: half height right = NDC +1");
    }

    #[test]
    fn camera_translation_moves_the_view_backwards() {
        // Camera orbits +2 in x; world origin must shift NEGATIVE in NDC.
        let cam = CameraMatrices::eval2d(Some(&camera(2.0, 0.0, 0.0, 1.0)), 1.0).unwrap();
        let p = cam.project_current(glam::Vec3::ZERO);
        assert!(
            (p.x + 0.4).abs() < 1e-4,
            "translation +2 (height 10) pushes origin to NDC -0.4, got {p:?}"
        );
    }

    #[test]
    fn zoom_grows_the_view() {
        let wide = CameraMatrices::eval2d(Some(&camera(0.0, 0.0, 0.0, 2.0)), 1.0).unwrap();
        let p = wide.project_current(glam::Vec3::new(
            (VIEW_HEIGHT_WORLD * 0.25) as f32,
            0.0,
            0.0,
        ));
        // zoom 2 → view shows 2× height; x = 2.5 world → NDC ±0.25
        assert!((p.x - 0.25).abs() < 1e-4, "got {p:?}");
    }

    #[test]
    fn rotation_rotates_world_into_view() {
        // rotate camera +90° (π/2): world +x maps to view -y (pointing down).
        let half_pi = std::f64::consts::FRAC_PI_2;
        let cam =
            CameraMatrices::eval2d(Some(&camera(0.0, 0.0, half_pi, 1.0)), 1.0).unwrap();
        let p = cam.project_current(glam::Vec3::new(
            (VIEW_HEIGHT_WORLD * 0.25) as f32,
            0.0,
            0.0,
        ));
        assert!(p.y < -0.2 && p.x.abs() < 1e-4, "got {p:?}");
    }

    #[test]
    fn depth_quantization_documents_its_units() {
        assert_eq!(quantize_view_depth(0.0f32).unwrap(), 0);
        assert_eq!(quantize_view_depth(1.0f32).unwrap(), 64);
        assert_eq!(quantize_view_depth(-2.0f32).unwrap(), -128);
        // depth stays strictly ordered after quantization at this scale
        assert!(quantize_view_depth(1.01).unwrap() > quantize_view_depth(1.0).unwrap());
        assert!(quantize_view_depth(f32::NAN).is_err());
        assert!(quantize_view_depth(f32::INFINITY).is_err());
        assert!(quantize_view_depth(1e30).is_err());
    }

    #[test]
    fn aspect_validation_is_strict() {
        assert!(CameraMatrices::eval2d(None, 0.0).is_err());
        assert!(CameraMatrices::eval2d(None, -1.0).is_err());
        assert!(CameraMatrices::eval2d(None, f64::NAN).is_err());
        assert!(
            CameraMatrices::eval2d(Some(&camera(0.0, 0.0, 0.0, f64::NAN)), 1.0).is_err()
        );
        assert!(CameraMatrices::eval2d(Some(&camera(0.0, 0.0, 0.0, 0.0)), 1.0).is_err());
    }

    #[test]
    fn invalidation_clears_validity_and_cut_bumps_serial() {
        let mut h = HistoryState::fresh();
        assert!(!h.valid);
        h.valid = true;
        h.cut_serial = 7;
        h.invalidate(&HistoryInvalidation::ProjectionChange);
        assert!(!h.valid);
        assert_eq!(h.cut_serial, 7, "non-cut invalidations must not bump the serial");
        h.invalidate(&HistoryInvalidation::CameraCut);
        assert_eq!(h.cut_serial, 8, "cut bumps the serial");
    }

    #[test]
    fn invalidation_reasons_are_exhaustive_in_tests() {
        // Every reason must invalidate (the list IS the contract).
        let reasons = [
            HistoryInvalidation::SceneLoad,
            HistoryInvalidation::DiscontinuousSeek,
            HistoryInvalidation::Teleport,
            HistoryInvalidation::CameraCut,
            HistoryInvalidation::ProjectionChange,
            HistoryInvalidation::TopologyDiscontinuity,
            HistoryInvalidation::TargetResize,
        ];
        for r in reasons {
            let mut h = HistoryState::fresh();
            h.valid = true;
            h.invalidate(&r);
            assert!(!h.valid, "{r:?} must invalidate");
        }
    }
}
