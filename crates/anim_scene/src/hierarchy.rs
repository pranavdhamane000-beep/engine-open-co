//! Parent validation, traversal order and world transforms (spec §13.7
//! "Hierarchy evaluation").
//!
//! * validate unique IDs and parent references, REJECT cycles
//! * visit roots in NodeId order, children in persisted sibling order
//!   (`anim_project` stores both as NodeId-ordered BTreeMaps)
//! * world = parent_world * local; unparented nodes use local
//! * reject nonfinite matrices (`InvalidNumericValue`)
//! * draw extraction keys are provided SEPARATELY from hierarchy order
//!   (see `snapshot::draw_order`)

use crate::error::{SceneError, SceneResult};
use crate::properties::evaluate_property;
use anim_core::{FramePos, NodeId, Rational};
use anim_project::{NodeData, PropertyValue, SceneData};

/// Row-major 2D affine matrix `[[f64; 3]; 3]` (spec §13.8): the bottom row
/// is always [0, 0, 1]; points are `world * column_vector`.
pub type Mat3 = [[f64; 3]; 3];

pub const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Matrix multiply a*b (row-major).
pub fn mat_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [[0.0; 3]; 3];
    for r in 0..3 {
        for c in 0..3 {
            out[r][c] = a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c];
        }
    }
    out
}

/// Transform the point (x, y); returns (x', y', w') with w' always 1 for
/// affine matrices.
pub fn mat_apply(m: &Mat3, x: f64, y: f64) -> (f64, f64) {
    (m[0][0] * x + m[0][1] * y + m[0][2], m[1][0] * x + m[1][1] * y + m[1][2])
}

pub const PROPERTY_TRANSLATION_X: &str = "translation_x";
pub const PROPERTY_TRANSLATION_Y: &str = "translation_y";
pub const PROPERTY_ROTATION: &str = "rotation";
pub const PROPERTY_SCALE_X: &str = "scale_x";
pub const PROPERTY_SCALE_Y: &str = "scale_y";
pub const PROPERTY_Z: &str = "z";
pub const PROPERTY_OPACITY: &str = "opacity";
pub const PROPERTY_VISIBLE: &str = "visible";
pub const PROPERTY_CAMERA: &str = "camera";
pub const PROPERTY_CAMERA_ZOOM: &str = "camera_zoom";

/// A canonical float property read from the node's track (defaults per
/// ADR-0008 when the name is ABSENT from the node).
pub(crate) fn float_property(
    node: &NodeData,
    name: &str,
    default: f64,
    t: &Rational,
) -> SceneResult<f64> {
    let v = match node.properties.get(name) {
        Some(track) => evaluate_property(track, t, name)?,
        None => return Ok(default),
    };
    match v {
        PropertyValue::Float(f) => {
            if !f.is_finite() {
                return Err(SceneError::InvalidNumericValue(format!(
                    "node {}: property '{name}' is not finite",
                    node.name
                )));
            }
            Ok(f)
        }
        other => Err(SceneError::InvalidPropertyKeys(format!(
            "property '{name}': expected Float, found {other:?}"
        ))),
    }
}

pub(crate) fn bool_property(
    node: &NodeData,
    name: &str,
    default: bool,
    t: &Rational,
) -> SceneResult<bool> {
    let v = match node.properties.get(name) {
        Some(track) => evaluate_property(track, t, name)?,
        None => return Ok(default),
    };
    match v {
        PropertyValue::Bool(b) => Ok(b),
        other => Err(SceneError::InvalidPropertyKeys(format!(
            "property '{name}': expected Bool, found {other:?}"
        ))),
    }
}

/// Static camera flag: the track DEFAULT decides (ADR-0008 — a camera is a
/// declared role, not a time-varying key).
pub(crate) fn is_camera(node: &NodeData) -> bool {
    matches!(
        node.properties.get(PROPERTY_CAMERA).and_then(|t| t.default.clone()),
        Some(PropertyValue::Bool(true))
    )
}

/// Local transform = T * R * S (translate, rotate, scale composition).
pub(crate) fn local_transform(node: &NodeData, t: &Rational) -> SceneResult<Mat3> {
    let tx = float_property(node, PROPERTY_TRANSLATION_X, 0.0, t)?;
    let ty = float_property(node, PROPERTY_TRANSLATION_Y, 0.0, t)?;
    let rot = float_property(node, PROPERTY_ROTATION, 0.0, t)?;
    let sx = float_property(node, PROPERTY_SCALE_X, 1.0, t)?;
    let sy = float_property(node, PROPERTY_SCALE_Y, 1.0, t)?;
    let (sin, cos) = rot.sin_cos();
    let m = [[cos * sx, -sin * sy, tx], [sin * sx, cos * sy, ty], [0.0, 0.0, 1.0]];
    require_finite_mat(&m, &node.name, "local")?;
    Ok(m)
}

pub(crate) fn require_finite_mat(m: &Mat3, owner: &str, what: &str) -> SceneResult<()> {
    if m.iter().flatten().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(SceneError::InvalidNumericValue(format!(
            "node {owner}: {what} transform is not finite"
        )))
    }
}

/// Validated traversal plan: parents verified, roots recorded in NodeId
/// order. Returns the parent map (`MissingParent` on dangling parents,
/// `HierarchyCycle` on any parent cycle).
pub(crate) fn validate_parents(scene: &SceneData) -> SceneResult<()> {
    for (id, node) in &scene.nodes {
        if let Some(parent) = node.parent {
            let parent_data = scene.nodes.get(&parent).ok_or_else(|| {
                SceneError::MissingParent(id.to_string(), parent.to_string())
            })?;
            // walk the parent chain with a visited set (cycle rejection)
            let mut cursor = parent_data.parent;
            let mut visited = 0usize;
            while let Some(next) = cursor {
                if next == *id {
                    return Err(SceneError::HierarchyCycle(id.to_string()));
                }
                visited += 1;
                if visited > scene.nodes.len() {
                    return Err(SceneError::HierarchyCycle(id.to_string()));
                }
                cursor = scene.nodes.get(&next).and_then(|n| n.parent);
            }
        }
    }
    Ok(())
}

/// Depth-first traversal: roots in NodeId order, children in persisted
/// (BTreeMap) sibling order. `visited` receives nodes parent-first so the
/// caller can compute world = parent_world * local in ONE pass.
pub(crate) fn traverse(scene: &SceneData) -> Vec<NodeId> {
    let mut order = Vec::with_capacity(scene.nodes.len());
    let children_of = |parent: Option<NodeId>| -> Vec<NodeId> {
        scene.nodes.iter().filter(|(_, n)| n.parent == parent).map(|(id, _)| *id).collect()
    };
    let mut stack: Vec<NodeId> = children_of(None).into_iter().rev().collect();
    // DFS with explicit explicit-stack that preserves sibling order:
    // unparented roots first, then each node's own children BEFORE the
    // next sibling (depth-first parent-first order).
    while let Some(id) = stack.pop() {
        order.push(id);
        let mut kids = children_of(Some(id));
        kids.reverse();
        stack.extend(kids);
    }
    order
}

/// The full evaluation of one node's LOCAL properties at time `t`
/// (ADR-0008 canonical set). Returns the float fields used by snapshots.
pub(crate) struct LocalPose {
    pub transform: Mat3,
    pub z: f64,
    pub opacity: f64,
    pub visible: bool,
}

pub(crate) fn local_pose(node: &NodeData, t: &Rational) -> SceneResult<LocalPose> {
    let transform = local_transform(node, t)?;
    let z = float_property(node, PROPERTY_Z, 0.0, t)?;
    // opacity evaluates in [0, 1] (clamped, ADR-0008)
    let opacity = float_property(node, PROPERTY_OPACITY, 1.0, t)?.clamp(0.0, 1.0);
    let visible = bool_property(node, PROPERTY_VISIBLE, true, t)?;
    Ok(LocalPose { transform, z, opacity, visible })
}

/// Unique-ID + parent validation entry (spec §13.5 step 1). Unique IDs are
/// a TYPE-LEVEL proof with BTreeMap keys (duplicate ids cannot exist here);
/// parent references and cycles are checked explicitly — the §13.11
/// DuplicateNode variant remains for callers that must report payloads
/// (e.g. imported JSON going through a non-BTree path in later WPs).
pub(crate) fn validate_scene(scene: &SceneData) -> SceneResult<()> {
    validate_parents(scene)
}

/// Whole-frame time bases (spec §13.6): frame n owns [time_of(n), time_of(n+1)).
pub(crate) fn time_of(pos: FramePos, fps: anim_core::FrameRate) -> SceneResult<Rational> {
    fps.time_of(pos).map_err(SceneError::from)
}

/// The time bases for whole frame n (start) and n+1 (exclusive end).
pub(crate) fn frame_window(
    fps: anim_core::FrameRate,
    frame: i64,
) -> SceneResult<(Rational, Rational)> {
    let start = fps.time_of(FramePos::new(frame, Rational::zero())?)?;
    let end = fps.time_of(FramePos::new(
        frame
            .checked_add(1)
            .ok_or_else(|| SceneError::TimeOverflow("frame window end overflow".into()))?,
        Rational::zero(),
    )?)?;
    Ok((start, end))
}
