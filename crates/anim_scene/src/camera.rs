//! Camera evaluation and cut markers (spec §13.5 step 5).
//!
//! Camera model (ADR-0008 MVP):
//!   * a CAMERA is a node whose `camera` property track DEFAULT is Bool(true)
//!     (a declared role, not time-varying)
//!   * camera ZOOM is evaluated from `camera_zoom` (default 1.0)
//!   * the camera's center = its evaluated local translation (cameras are
//!     MVP top-level; parents compose through the world transform anyway)
//!   * CUTS are `MarkerKind::Cut` markers whose NAME is the camera node's
//!     NAME; at position p the active camera is the one named by the LATEST
//!     cut with `at <= p`; before any cut → the lowest-NodeId camera
//!   * a cut naming an unknown/non-camera node is a TYPED
//!     `UnknownCamera` error — never skipped silently (§13.11)

use crate::error::{SceneError, SceneResult};
use crate::hierarchy::{
    float_property, is_camera, PROPERTY_CAMERA_ZOOM, PROPERTY_TRANSLATION_X,
    PROPERTY_TRANSLATION_Y,
};
use crate::snapshot::{cut_markers, EvaluatedCamera};
use anim_core::{FramePos, NodeId, Rational};
use anim_project::SceneData;

/// All cameras at `t` with the cut-resolved `active` flags.
pub(crate) fn evaluate_cameras(
    scene: &SceneData,
    position: FramePos,
    t: &Rational,
) -> SceneResult<Vec<EvaluatedCamera>> {
    let camera_nodes: Vec<(NodeId, &anim_project::NodeData)> =
        scene.nodes.iter().filter(|(_, n)| is_camera(n)).map(|(id, n)| (*id, n)).collect();
    if camera_nodes.is_empty() {
        return Ok(Vec::new());
    }

    // resolve the ACTIVE camera once for the position (cut rule above)
    let active_id = resolve_active_camera(scene, &camera_nodes, position)?;

    let mut out = Vec::with_capacity(camera_nodes.len());
    for (id, node) in &camera_nodes {
        let center_x = float_property(node, PROPERTY_TRANSLATION_X, 0.0, t)?;
        let center_y = float_property(node, PROPERTY_TRANSLATION_Y, 0.0, t)?;
        let zoom = float_property(node, PROPERTY_CAMERA_ZOOM, 1.0, t)?;
        // rotation reuses the node's basic pose property (same canonical name)
        let rotation = float_property(node, crate::hierarchy::PROPERTY_ROTATION, 0.0, t)?;
        if !(center_x.is_finite()
            && center_y.is_finite()
            && zoom.is_finite()
            && rotation.is_finite())
        {
            return Err(SceneError::InvalidNumericValue(format!(
                "camera {id}: nonfinite parameter"
            )));
        }
        out.push(EvaluatedCamera {
            id: *id,
            center_x,
            center_y,
            rotation,
            zoom,
            active: *id == active_id,
        });
    }
    Ok(out)
}

fn resolve_active_camera(
    scene: &SceneData,
    camera_nodes: &[(NodeId, &anim_project::NodeData)],
    position: FramePos,
) -> SceneResult<NodeId> {
    // default BEFORE any cut: the lowest-NodeId camera
    let default = camera_nodes.iter().map(|(id, _)| *id).min().unwrap();

    let cuts = cut_markers(&scene.markers);
    // the latest cut at or before the position
    let mut latest: Option<&anim_project::SceneMarker> = None;
    for cut in &cuts {
        if cut.at <= position {
            latest = Some(cut);
        } else {
            break; // sorted cut list → everything later is beyond position
        }
    }

    let Some(marker) = latest else {
        return Ok(default);
    };
    // cuts name the camera NODE NAME
    let named = camera_nodes
        .iter()
        .find(|(_, node)| node.name == marker.name)
        .map(|(id, _)| *id)
        .ok_or_else(|| SceneError::UnknownCamera(marker.name.clone()))?;
    Ok(named)
}

/// Camera-role + zoom static sanity check surfaced to validation callers.
pub(crate) fn validate_camera_roles(scene: &SceneData) -> SceneResult<()> {
    // every Cut marker must name an existing camera node (typed miss)
    let has_camera = scene.nodes.iter().any(|(_, n)| is_camera(n));
    if !has_camera && cut_markers(&scene.markers).is_empty() {
        return Ok(());
    }
    for cut in cut_markers(&scene.markers) {
        let named = scene.nodes.iter().find(|(_, n)| n.name == cut.name && is_camera(n));
        if named.is_none() {
            return Err(SceneError::UnknownCamera(cut.name.clone()));
        }
    }
    Ok(())
}
