//! Track/layer blending and basic pose/cel selection (spec §13.5 step 3).
//!
//! MVP semantics (ADR-0008): `TrackData` entries are priorities over
//! (target node, property name) pairs. When SEVERAL tracks drive the same
//! pair, the highest `layer` wins in authored priority order and the
//! others are ignored — an override blend, not a numeric mix (numeric
//! layer mixing is WP05+ once poses exist). Single-track properties
//! evaluate through `properties::evaluate_property` unchanged.

use crate::error::SceneResult;
use crate::hierarchy::{
    float_property, PROPERTY_ROTATION, PROPERTY_SCALE_X, PROPERTY_SCALE_Y,
};
use crate::properties::evaluate_property;
use anim_core::{NodeId, Rational};
use anim_project::{NodeData, PropertyValue, SceneData, TrackData};

/// Which tracks drive (node, property), sorted by layer ascending.
pub(crate) fn tracks_for<'a>(
    scene: &'a SceneData,
    node: NodeId,
    property: &str,
) -> Vec<&'a TrackData> {
    let mut own: Vec<&TrackData> = scene
        .tracks
        .values()
        .filter(|t| t.target == node && t.property == property)
        .collect();
    own.sort_by_key(|t| t.layer);
    own
}

/// The EFFECTIVE property value at the evaluated position, honoring layer
/// override: `None` when no track drives the pair at all (property state
/// is then the node's own track/default, which callers already handle).
pub(crate) fn layer_override_value(
    scene: &SceneData,
    node: NodeId,
    property: &str,
    t: &Rational,
) -> SceneResult<Option<PropertyValue>> {
    let tracks = tracks_for(scene, node, property);
    match tracks.last() {
        Some(top) => {
            // the winning layer's track: the KEYS live on the target node
            let node_data: &NodeData = scene
                .nodes
                .get(&top.target)
                .expect("document validation requires track targets to exist");
            match node_data.properties.get(property) {
                Some(track) => Ok(Some(evaluate_property(track, t, property)?)),
                None => Ok(None), // track drifts nothing; no override
            }
        }
        None => Ok(None),
    }
}

/// Basic pose/cel selection (spec §13.5 step 5): a sprite-cel index is the
/// evaluated Int property `cel` (absent → 0). Rotation+scale make the
/// "basic pose" used by hierarchy composition.
pub const PROPERTY_CEL: &str = "cel";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BasicPose {
    pub rotation: f64,
    pub scale_x: f64,
    pub scale_y: f64,
    /// Sprite-cel index for cel-based assets (0 = first cel).
    pub cel: i64,
}

pub(crate) fn basic_pose(node: &NodeData, t: &Rational) -> SceneResult<BasicPose> {
    let rotation = float_property(node, PROPERTY_ROTATION, 0.0, t)?;
    let scale_x = float_property(node, PROPERTY_SCALE_X, 1.0, t)?;
    let scale_y = float_property(node, PROPERTY_SCALE_Y, 1.0, t)?;
    let cel = match node.properties.get(PROPERTY_CEL) {
        Some(track) => match evaluate_property(track, t, PROPERTY_CEL)? {
            PropertyValue::Int(i) => i,
            other => {
                return crate::error::SceneResult::Err(
                    crate::error::SceneError::InvalidPropertyKeys(format!(
                        "property 'cel': expected Int, found {other:?}"
                    )),
                )
            }
        },
        None => 0,
    };
    Ok(BasicPose { rotation, scale_x, scale_y, cel })
}
