//! Draw extraction, validation and total ordering (spec §15.7, §15.8).
//!
//! `extract_frame` is a PURE function of (immutable scene snapshot,
//! immutable simulation frame, caller-owned previous state, quality).
//! It builds one `RenderFrame` per call:
//!   1. Resolve the cut-resolved active camera (exactly one or none).
//!   2. Validate finiteness (transforms, z, opacity, depth) — any
//!      violation fails extraction with a typed error. No clamping,
//!      no silent fallbacks.
//!   3. Build items: scene nodes (asset-bearing, visible), simulation
//!      particles (transparent, premultiplied) and spring bodies (the
//!      sim transform REPLACES the node's world transform).
//!   4. Verify unique (entity, draw_index) sort identities.
//!   5. Stable total sort by (layer_order ASC, quantized view depth
//!      DESC, material_pass ASC, entity ASC, draw_index ASC).
//!
//! ADR-0010 records the additive fields beyond the §15.8 sketch
//! (`opacity`, `velocity_valid`, `camera_id`) and the particle
//! identity choice (sim-derived particle id → synthetic NodeId).

use crate::alpha::{validate_alpha_mode, AlphaMode};
use crate::camera::{quantize_view_depth, CameraMatrices, HistoryState};
use crate::error::{RenderError, RenderResult};
use crate::graph::{MATERIAL_ORDERED_2D, MATERIAL_TRANSPARENT};
use crate::RenderQuality;
use anim_scene::hierarchy::Mat3;
use anim_scene::snapshot::{EvaluatedNode, SceneSnapshot};
use glam::{Mat4, Vec3, Vec4};
use std::collections::BTreeMap;

/// Layer quantization for the project draw-order z (documented units:
/// 1 quantum = 1/4096 world z; z ∈ [-524288, 524288] fits i32).
const LAYER_QUANTUM: f64 = 4096.0;

/// §15.8 DrawItem (+ the ADR-0010 additive fields). Matrices are glam
/// (column-major, f32) and bytemuck-castable for instance upload in the
/// GPU half.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawItem {
    pub entity: anim_core::NodeId,
    pub draw_index: u32,
    pub layer_order: i32,
    pub material_pass: u16,
    pub view_depth: f32,
    pub current_transform: Mat4,
    /// Used by the velocity pass (GPU half) — read via instance upload.
    #[allow(dead_code)]
    pub previous_transform: Mat4,
    pub alpha_mode: AlphaMode,
    /// ADDITIVE (ADR-0010): authored node opacity in [0, 1] folded into
    /// the instance data at draw time.
    pub opacity: f32,
    /// ADDITIVE (ADR-0010): linear straight-authored tint (rgb·a applied
    /// at draw time per the item's alpha mode). Extraction uses white ×
    /// node opacity; crafted frames may carry arbitrary validated tints.
    pub tint: [f32; 4],
    /// ADDITIVE (ADR-0010): this item's velocity pair is trustworthy.
    pub velocity_valid: bool,
}

/// §15.8 RenderFrame (+ `camera_id`, ADR-0010 additive: the renderer
/// needs the NEW active-camera identity for its post-submission
/// `PreviousFrameState` bookkeeping, and the §15.8 sketch omits it).
#[derive(Debug, Clone, PartialEq)]
pub struct RenderFrame {
    pub revision: anim_core::Revision,
    pub tick: anim_sim::SimTick,
    pub items: Vec<DrawItem>,
    pub camera: CameraMatrices,
    pub camera_id: Option<anim_core::NodeId>,
    pub cut: bool,
    pub quality: RenderQuality,
}

/// Previous-frame state the CALLER (renderer) owns and updates only
/// after a successful submission (§15.7).
#[derive(Debug, Clone)]
pub struct PreviousFrameState {
    pub history: HistoryState,
    /// Current viewport aspect (updated at resize; a change invalidates
    /// history as ProjectionChange — enforced by the renderer).
    pub viewport_aspect: f64,
    /// Active camera id of the last valid frame (cut detection).
    pub previous_camera_id: Option<anim_core::NodeId>,
    /// Last-submitted scene-node world transforms (BTreeMap — the sort
    /// prohibition concerns OUTPUT ORDER; storage stays deterministic).
    pub previous_node_transforms: BTreeMap<anim_core::NodeId, Mat4>,
}

impl PreviousFrameState {
    /// Fresh state: no history at all.
    pub fn fresh() -> Self {
        Self {
            history: HistoryState::fresh(),
            viewport_aspect: 1.0,
            previous_camera_id: None,
            previous_node_transforms: BTreeMap::new(),
        }
    }

    /// The post-submission update hook (§15.7): records THIS frame's
    /// camera pair and transforms, validates history for the NEXT
    /// extraction, and commits the cut serial for THIS frame's cut.
    /// Callers that invalidated for non-cut reasons (load, seek, resize,
    /// …) must call `history.invalidate(reason)` BEFORE rendering.
    pub fn record_successful_submission(
        &mut self,
        frame: &RenderFrame,
        new_generation: u64,
    ) {
        self.history.previous_view_proj = frame.camera.current_view_proj;
        self.history.previous_tick = Some(frame.tick);
        self.history.target_generation = new_generation;
        self.history.valid = true;
        if frame.cut {
            self.history.cut_serial = self.history.cut_serial.wrapping_add(1);
        }
        self.previous_camera_id = frame.camera_id;
        self.previous_node_transforms.clear();
        for item in &frame.items {
            self.previous_node_transforms.insert(item.entity, item.current_transform);
        }
    }
}

/// Convert the engine's row-major 2D affine (p' = M·[x, y, 1]ᵀ,
/// translation in [0][2]/[1][2]) into a column-major glam Mat4.
/// The z axis passes through (2D plane coordinates).
pub fn mat4_from_mat3(m: &Mat3) -> Mat4 {
    Mat4::from_cols(
        Vec4::new(m[0][0] as f32, m[1][0] as f32, 0.0, 0.0),
        Vec4::new(m[0][1] as f32, m[1][1] as f32, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(m[0][2] as f32, m[1][2] as f32, 0.0, 1.0),
    )
}

/// Pure translation matrix (particles/bodies carry exact positions).
fn translation_mat4(p: [f32; 3]) -> Mat4 {
    Mat4::from_translation(Vec3::from_array(p))
}

/// Particle draw transform: position · rotation · UNIFORM sprite size
/// (unit quad → world-space sprite box). Documentation/pinned in
/// ADR-0010: particles carry no authored anchor offset (center = pos).
fn particle_transform(p: [f32; 3], rotation: f32, size: f32) -> Mat4 {
    translation_mat4(p)
        * Mat4::from_rotation_z(rotation)
        * Mat4::from_scale(Vec3::splat(size))
}

/// §15.9 extract_frame. Fails typed on: non-finite transforms, invalid
/// opacity, invalid alpha cutoffs, non-finite view depth, duplicate
/// sort identities, non-finite cameras/viewport data.
pub fn extract_frame(
    scene: &SceneSnapshot,
    sim: &anim_sim::state::SimulationFrame,
    previous: &PreviousFrameState,
    quality: RenderQuality,
) -> RenderResult<RenderFrame> {
    // ---- 1. active camera (exactly one or none) ----
    let actives: Vec<&anim_scene::snapshot::EvaluatedCamera> =
        scene.cameras.iter().filter(|c| c.active).collect();
    if actives.len() > 1 {
        return Err(RenderError::InvalidSnapshot(format!(
            "{} active cameras at once (cut resolution is single-shot)",
            actives.len()
        )));
    }
    let active = actives.first().copied();
    let camera = CameraMatrices::eval2d(active, previous.viewport_aspect)?;

    // ---- 2. cut detection (identity change against the last valid frame) ----
    let active_id = active.map(|c| c.id);
    let cut = previous.history.valid
        && previous.previous_camera_id.is_some()
        && previous.previous_camera_id != active_id;

    // ---- 3. material items ----
    let mut items: Vec<DrawItem> = Vec::new();

    // Body lookup: a sim body REPLACES its scene node's transform.
    let mut bodies_by_id: BTreeMap<anim_core::NodeId, &anim_sim::state::BodyTransform> =
        BTreeMap::new();
    for b in &sim.bodies {
        if !b.current.iter().all(|v| v.is_finite())
            || !b.previous.iter().all(|v| v.is_finite())
        {
            return Err(RenderError::NaNTransform(format!("body {}", b.id), 0));
        }
        bodies_by_id.insert(b.id, b);
    }
    let history_valid = previous.history.valid;

    for node in scene.nodes.iter() {
        if !node.visible || node.asset.is_none() {
            continue; // nothing to draw (MVP: asset-bearing + visible only)
        }
        validate_node_finiteness(node)?;
        let entity = node.id;
        let layer_order = quantize_layer(node.z, entity)?;

        // Transform override: physical bodies follow the simulation.
        let (current, previous_xform, velocity_valid, view_depth) = match bodies_by_id
            .remove(&entity)
        {
            Some(b) => (
                translation_mat4(b.current),
                translation_mat4(b.previous),
                history_valid, // sim-tracked exact pair
                b.current[2],
            ),
            None => {
                let current = mat4_from_mat3(&node.world_transform);
                let previous_xform = previous
                    .previous_node_transforms
                    .get(&entity)
                    .copied()
                    .unwrap_or(current);
                // A node without a recorded previous is NEW (topology
                // change) → no velocity. History-invalid → none either.
                let has_previous = previous.previous_node_transforms.contains_key(&entity);
                (current, previous_xform, history_valid && has_previous, 0.0)
            }
        };
        if !current.is_finite() || !previous_xform.is_finite() {
            return Err(RenderError::NaNTransform(entity.to_string(), 0));
        }

        items.push(DrawItem {
            entity,
            draw_index: 0,
            layer_order,
            material_pass: MATERIAL_ORDERED_2D,
            view_depth,
            current_transform: current,
            previous_transform: previous_xform,
            // MVP material contract: unlit textured quad, straight-alpha
            // decode → premultiplied in the ordered-2D pass. ADR-0010.
            alpha_mode: AlphaMode::Straight,
            opacity: node.opacity as f32,
            tint: [1.0, 1.0, 1.0, node.opacity as f32],
            velocity_valid,
        });
    }
    // Any remaining sim body whose node produced no item (node missing,
    // invisible or asset-less) is a topology discontinuity owned by the
    // scene — the sim cannot invent drawables; the CALLER invalidates
    // history through TopologyDiscontinuity when topology changes.

    // ---- particles: TRANSPARENT pass, premultiplied, sim identity ----
    for p in &sim.particles {
        if !p.position.iter().all(|v| v.is_finite())
            || !p.previous_position.iter().all(|v| v.is_finite())
        {
            return Err(RenderError::NaNTransform(format!("particle {}", p.id), 0));
        }
        // Particle identity = the sim's derived id (unique per frame by
        // construction; collisions fail the explicit duplicate check).
        let entity = anim_core::NodeId::from_u128(p.id);
        // Freshly spawned (age <= 1): previous_position is fabricated
        // (spawn origin), NOT a real previous frame → invalid velocity.
        let velocity_valid = history_valid && p.age_ticks > 1;
        items.push(DrawItem {
            entity,
            draw_index: 0,
            layer_order: crate::graph::PARTICLE_LAYER_ORDER,
            material_pass: MATERIAL_TRANSPARENT,
            view_depth: p.position[2],
            current_transform: particle_transform(p.position, p.rotation, p.size),
            previous_transform: particle_transform(p.previous_position, p.rotation, p.size),
            alpha_mode: AlphaMode::Premultiplied,
            opacity: 1.0,
            tint: [1.0, 1.0, 1.0, 1.0],
            velocity_valid,
        });
    }

    // ---- 4. unique (entity, draw_index) sort identities ----
    let mut keys: Vec<(anim_core::NodeId, u32)> =
        items.iter().map(|i| (i.entity, i.draw_index)).collect();
    keys.sort_unstable();
    let dup =
        keys.iter().zip(keys.iter().skip(1)).find_map(|(a, b)| (a == b).then_some(*a));
    if let Some((entity, index)) = dup {
        return Err(RenderError::DuplicateSortIdentity(entity.to_string(), index));
    }

    // ---- validate alpha modes ----
    for item in &items {
        validate_alpha_mode(item.alpha_mode)?;
    }

    // ---- 5. stable total sort (§15.7) ----
    sort_items_total(&mut items)?;

    Ok(RenderFrame {
        revision: scene.revision,
        tick: sim.tick,
        items,
        camera: CameraMatrices {
            current_view_proj: camera.current_view_proj,
            previous_view_proj: if previous.history.valid {
                previous.history.previous_view_proj
            } else {
                camera.current_view_proj
            },
        },
        camera_id: active_id,
        cut,
        quality,
    })
}

/// Validate one evaluated node's finite inputs (transform, z, opacity).
fn validate_node_finiteness(node: &EvaluatedNode) -> RenderResult<()> {
    for row in 0..3 {
        for col in 0..3 {
            if !node.world_transform[row][col].is_finite() {
                return Err(RenderError::NaNTransform(node.id.to_string(), 0));
            }
        }
    }
    if !node.z.is_finite() {
        return Err(RenderError::NaNTransform(node.id.to_string(), 0));
    }
    if !node.opacity.is_finite() || !(0.0..=1.0).contains(&node.opacity) {
        return Err(RenderError::InvalidOpacity(node.opacity));
    }
    Ok(())
}

/// Quantize the project draw-order z into layer_order (documented units).
fn quantize_layer(z: f64, entity: anim_core::NodeId) -> RenderResult<i32> {
    let scaled = z * LAYER_QUANTUM;
    if !scaled.is_finite() || scaled > i32::MAX as f64 || scaled < i32::MIN as f64 {
        return Err(RenderError::NaNTransform(entity.to_string(), 0));
    }
    Ok(scaled.round() as i32)
}

/// Stable total comparator (§15.7): (layer_order ASC, quantized view
/// depth DESC, material_pass ASC, entity ASC, draw_index ASC). Rust's
/// `sort_by` is stable and this comparator is a strict weak order over
/// every field pair → total, hash-free, timing-free ordering.
pub fn sort_items_total(items: &mut [DrawItem]) -> RenderResult<()> {
    // Quantize depths FIRST so a failure cannot reorder anything.
    let quantized: RenderResult<Vec<i32>> =
        items.iter().map(|i| quantize_view_depth(i.view_depth)).collect();
    let quantized = quantized?;

    let mut staged: Vec<(i32, DrawItem)> =
        items.iter().copied().zip(quantized).map(|(item, depth)| (depth, item)).collect();
    staged.sort_by(|a, b| {
        a.1.layer_order
            .cmp(&b.1.layer_order)
            .then_with(|| a.0.cmp(&b.0).reverse()) // depth DESCENDING
            .then_with(|| a.1.material_pass.cmp(&b.1.material_pass))
            .then_with(|| a.1.entity.cmp(&b.1.entity))
            .then_with(|| a.1.draw_index.cmp(&b.1.draw_index))
    });
    for (slot, (_, item)) in items.iter_mut().zip(staged) {
        *slot = item;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anim_core::{AssetId, FramePos, NodeId, Rational, Revision, SceneId};
    use anim_scene::hierarchy::IDENTITY;
    use anim_sim::clock::SimTick;

    fn node(
        id: NodeId,
        z: f64,
        opacity: f64,
        visible: bool,
        with_asset: bool,
    ) -> EvaluatedNode {
        EvaluatedNode {
            id,
            parent: None,
            world_transform: IDENTITY,
            z,
            opacity,
            visible,
            asset: if with_asset { Some(AssetId::from_u128(1)) } else { None },
        }
    }

    fn snapshot(
        nodes: Vec<EvaluatedNode>,
        cameras: Vec<anim_scene::snapshot::EvaluatedCamera>,
    ) -> SceneSnapshot {
        SceneSnapshot {
            scene_id: SceneId::new_v4(),
            revision: Revision(7),
            position: FramePos::new(0, Rational::zero()).unwrap(),
            time: Rational::zero(),
            nodes: nodes.into(),
            cameras: cameras.into(),
            audio_cues: Vec::new().into(),
            simulation_inputs: Vec::new().into(),
            events: Vec::new().into(),
        }
    }

    fn active_camera(id: NodeId) -> anim_scene::snapshot::EvaluatedCamera {
        anim_scene::snapshot::EvaluatedCamera {
            id,
            center_x: 0.0,
            center_y: 0.0,
            rotation: 0.0,
            zoom: 1.0,
            active: true,
        }
    }

    fn particle(
        id: u128,
        emitter: NodeId,
        position: [f32; 3],
        age: u64,
    ) -> anim_sim::state::ParticleDraw {
        anim_sim::state::ParticleDraw {
            id,
            emitter,
            position,
            previous_position: position,
            size: 0.2,
            rotation: 0.0,
            age_ticks: age,
            age_frac: 0.5,
        }
    }

    fn sim_frame(
        tick: i64,
        particles: Vec<anim_sim::state::ParticleDraw>,
        bodies: Vec<anim_sim::state::BodyTransform>,
    ) -> anim_sim::state::SimulationFrame {
        anim_sim::state::SimulationFrame { tick: SimTick(tick), particles, bodies }
    }

    #[test]
    fn only_visible_asset_bearing_nodes_draw() {
        let a = NodeId::new_v4();
        let b = NodeId::new_v4();
        let c = NodeId::new_v4();
        let scene = snapshot(
            vec![
                node(a, 0.0, 1.0, true, true),
                node(b, 1.0, 1.0, false, true),
                node(c, 2.0, 1.0, true, false),
            ],
            Vec::new(),
        );
        let frame = extract_frame(
            &scene,
            &sim_frame(4, Vec::new(), Vec::new()),
            &PreviousFrameState::fresh(),
            RenderQuality::Preview,
        )
        .unwrap();
        assert_eq!(frame.items.len(), 1);
        assert_eq!(frame.items[0].entity, a);
        assert_eq!(frame.items[0].material_pass, MATERIAL_ORDERED_2D);
        assert_eq!(frame.items[0].alpha_mode, AlphaMode::Straight);
        assert_eq!(frame.revision.0, 7);
        assert_eq!(frame.tick, SimTick(4));
    }

    #[test]
    fn layer_order_quantizes_z_and_sorts_ascending() {
        let a = NodeId::new_v4();
        let b = NodeId::new_v4();
        let scene = snapshot(
            vec![node(a, 5.0, 1.0, true, true), node(b, -3.0, 1.0, true, true)],
            Vec::new(),
        );
        let frame = extract_frame(
            &scene,
            &sim_frame(1, Vec::new(), Vec::new()),
            &PreviousFrameState::fresh(),
            RenderQuality::Preview,
        )
        .unwrap();
        assert_eq!(frame.items.len(), 2);
        // backward z first (ASC layer order)
        assert_eq!(frame.items[0].entity, b);
        assert_eq!(frame.items[0].layer_order, -3 * 4096);
        assert_eq!(frame.items[1].entity, a);
        assert_eq!(frame.items[1].layer_order, 5 * 4096);
    }

    #[test]
    fn transparent_sort_is_depth_descending_within_a_layer() {
        let e = NodeId::new_v4();
        let far = particle(101, e, [0.0, 0.0, 4.0], 5);
        let near = particle(102, e, [1.0, 1.0, 1.0], 5);
        let scene = snapshot(Vec::new(), Vec::new());
        let frame = extract_frame(
            &scene,
            &sim_frame(2, vec![far, near], Vec::new()),
            &PreviousFrameState::fresh(),
            RenderQuality::Preview,
        )
        .unwrap();
        // same layer, same material: depth 4 FIRST (far back), then 1
        assert!(frame.items[0].view_depth > frame.items[1].view_depth);
        assert_eq!(frame.items[0].view_depth, 4.0);
        assert_eq!(frame.items[1].view_depth, 1.0);
        assert_eq!(frame.items[0].material_pass, MATERIAL_TRANSPARENT);
    }

    #[test]
    fn equal_depth_ties_fall_through_material_then_entity() {
        // same layer (particles), same depth → material asc, then entity asc
        let newer = NodeId::new_v4();
        let p_hi_entity = particle(901, newer, [0.0, 0.0, 2.5], 3);
        let p_lo_entity = particle(900, newer, [1.0, 1.0, 2.5], 3);
        let scene = snapshot(Vec::new(), Vec::new());
        let frame = extract_frame(
            &scene,
            &sim_frame(3, vec![p_lo_entity, p_hi_entity], Vec::new()),
            &PreviousFrameState::fresh(),
            RenderQuality::Preview,
        )
        .unwrap();
        // both candidates stay in particle band; entity 0x?? low first
        let e_ids: Vec<NodeId> = frame.items.iter().map(|i| i.entity).collect();
        assert_eq!(e_ids[0], NodeId::from_u128(900));
        assert_eq!(e_ids[1], NodeId::from_u128(901));
    }

    #[test]
    fn particles_render_in_the_top_layer_band() {
        let big_z_node = node(NodeId::new_v4(), 100_000.0, 1.0, true, true);
        let e = NodeId::new_v4();
        let scene = snapshot(vec![big_z_node], Vec::new());
        let frame = extract_frame(
            &scene,
            &sim_frame(1, vec![particle(5, e, [0.0; 3], 3)], Vec::new()),
            &PreviousFrameState::fresh(),
            RenderQuality::Preview,
        )
        .unwrap();
        assert_eq!(frame.items.len(), 2);
        // The node's layer (100000 × 4096) is still below the particle
        // band → node first, particle LAST (transparent on top).
        assert_eq!(frame.items[1].material_pass, MATERIAL_TRANSPARENT);
        assert_eq!(frame.items[0].material_pass, MATERIAL_ORDERED_2D);
        assert!(frame.items[1].layer_order >= crate::graph::PARTICLE_LAYER_ORDER);
    }

    #[test]
    fn duplicate_identity_fails_extraction() {
        let n = NodeId::new_v4();
        // a particle whose derived id collides with the node's identity
        let colliding = particle(n.0.as_u128(), n, [0.0; 3], 2);
        let scene = snapshot(vec![node(n, 0.0, 1.0, true, true)], Vec::new());
        let err = extract_frame(
            &scene,
            &sim_frame(1, vec![colliding], Vec::new()),
            &PreviousFrameState::fresh(),
            RenderQuality::Preview,
        )
        .unwrap_err();
        assert!(matches!(err, RenderError::DuplicateSortIdentity(_, _)));
    }

    #[test]
    fn invalid_node_data_fails_typed() {
        let bad_transform = node(NodeId::new_v4(), 0.0, 1.0, true, true);
        let mut bad = bad_transform.clone();
        bad.world_transform = [[1.0, 0.0, 0.0], [0.0, 1.0, f64::NAN], [0.0, 0.0, 1.0]];
        let scene = snapshot(vec![bad], Vec::new());
        assert!(matches!(
            extract_frame(
                &scene,
                &sim_frame(1, Vec::new(), Vec::new()),
                &PreviousFrameState::fresh(),
                RenderQuality::Preview
            )
            .unwrap_err(),
            RenderError::NaNTransform(_, _)
        ));

        let bad_opacity = node(NodeId::new_v4(), 0.0, 1.5, true, true);
        let scene = snapshot(vec![bad_opacity], Vec::new());
        assert!(matches!(
            extract_frame(
                &scene,
                &sim_frame(1, Vec::new(), Vec::new()),
                &PreviousFrameState::fresh(),
                RenderQuality::Preview
            )
            .unwrap_err(),
            RenderError::InvalidOpacity(_)
        ));

        let extreme_z = node(NodeId::new_v4(), 1e18, 1.0, true, true);
        let scene = snapshot(vec![extreme_z], Vec::new());
        assert!(matches!(
            extract_frame(
                &scene,
                &sim_frame(1, Vec::new(), Vec::new()),
                &PreviousFrameState::fresh(),
                RenderQuality::Preview
            )
            .unwrap_err(),
            RenderError::NaNTransform(_, _)
        ));

        let bad_particle = particle(7, NodeId::new_v4(), [f32::NAN, 0.0, 0.0], 2);
        let scene = snapshot(Vec::new(), Vec::new());
        assert!(matches!(
            extract_frame(
                &scene,
                &sim_frame(1, vec![bad_particle], Vec::new()),
                &PreviousFrameState::fresh(),
                RenderQuality::Preview
            )
            .unwrap_err(),
            RenderError::NaNTransform(_, _)
        ));
    }

    #[test]
    fn bodies_override_their_nodes_transform() {
        let n = NodeId::new_v4();
        let body = anim_sim::state::BodyTransform {
            id: n,
            current: [3.0, -1.0, 0.0],
            previous: [2.5, -1.0, 0.0],
        };
        let scene = snapshot(vec![node(n, 0.0, 1.0, true, true)], Vec::new());
        let frame = extract_frame(
            &scene,
            &sim_frame(9, Vec::new(), vec![body]),
            &PreviousFrameState::fresh(),
            RenderQuality::Preview,
        )
        .unwrap();
        let item = &frame.items[0];
        let col3 = item.current_transform.to_cols_array_2d()[3];
        assert_eq!((col3[0], col3[1]), (3.0, -1.0));
        let prev3 = item.previous_transform.to_cols_array_2d()[3];
        assert_eq!((prev3[0], prev3[1]), (2.5, -1.0));
    }

    #[test]
    fn nonfinite_body_fails_extraction() {
        let body = anim_sim::state::BodyTransform {
            id: NodeId::new_v4(),
            current: [f32::INFINITY, 0.0, 0.0],
            previous: [0.0; 3],
        };
        let scene = snapshot(Vec::new(), Vec::new());
        assert!(matches!(
            extract_frame(
                &scene,
                &sim_frame(1, Vec::new(), vec![body]),
                &PreviousFrameState::fresh(),
                RenderQuality::Preview
            )
            .unwrap_err(),
            RenderError::NaNTransform(_, _)
        ));
    }

    #[test]
    fn fresh_history_gives_no_valid_velocity_and_no_cut() {
        let n = NodeId::new_v4();
        let scene = snapshot(vec![node(n, 0.0, 1.0, true, true)], vec![active_camera(n)]);
        let frame = extract_frame(
            &scene,
            &sim_frame(1, Vec::new(), Vec::new()),
            &PreviousFrameState::fresh(),
            RenderQuality::Preview,
        )
        .unwrap();
        assert!(!frame.cut);
        assert!(!frame.items[0].velocity_valid);
        assert_eq!(frame.camera_id, Some(n));
    }

    #[test]
    fn multiple_active_cameras_fail() {
        let id = NodeId::new_v4();
        let scene =
            snapshot(Vec::new(), vec![active_camera(id), active_camera(NodeId::new_v4())]);
        assert!(matches!(
            extract_frame(
                &scene,
                &sim_frame(1, Vec::new(), Vec::new()),
                &PreviousFrameState::fresh(),
                RenderQuality::Preview
            )
            .unwrap_err(),
            RenderError::InvalidSnapshot(_)
        ));
    }

    #[test]
    fn velocity_validity_follows_history_and_spawnage() {
        let body_node = NodeId::new_v4();
        let emitter = NodeId::new_v4();
        let scene = snapshot(vec![node(body_node, 0.0, 1.0, true, true)], Vec::new());
        let sim = sim_frame(
            2,
            vec![
                particle(11, emitter, [0.0; 3], 2), // mature
                particle(12, emitter, [1.0; 3], 1), // freshly spawned
            ],
            vec![anim_sim::state::BodyTransform {
                id: body_node,
                current: [0.0; 3],
                previous: [0.0; 3],
            }],
        );

        // history invalid → NOTHING has valid velocity
        let fresh = extract_frame(
            &scene,
            &sim,
            &PreviousFrameState::fresh(),
            RenderQuality::Preview,
        )
        .unwrap();
        assert!(fresh.items.iter().all(|i| !i.velocity_valid));
        assert!(!fresh.cut);

        // record a successful submission → next extraction has history
        let mut previous = PreviousFrameState::fresh();
        previous.record_successful_submission(&fresh, 0);
        previous.previous_node_transforms.insert(body_node, glam::Mat4::IDENTITY);

        let valid = extract_frame(&scene, &sim, &previous, RenderQuality::Preview).unwrap();
        let by_entity: BTreeMap<NodeId, bool> =
            valid.items.iter().map(|i| (i.entity, i.velocity_valid)).collect();
        assert!(by_entity[&anim_core::NodeId::from_u128(11)]);
        assert!(
            !by_entity[&anim_core::NodeId::from_u128(12)],
            "freshly spawned must be velocity-invalid"
        );
        assert!(by_entity[&body_node], "bodies carry sim-tracked pairs");
        assert!(!valid.cut, "same camera id → no cut");
    }

    #[test]
    fn camera_identity_change_is_a_cut() {
        let cam_a = active_camera(NodeId::new_v4());
        let cam_b = active_camera(NodeId::new_v4());

        // Previous: valid history rendered cam_a.
        let mut previous = PreviousFrameState::fresh();
        previous.history.valid = true;
        previous.previous_camera_id = Some(cam_a.id);
        previous.history.previous_tick = Some(SimTick(1));

        let scene = snapshot(Vec::new(), vec![cam_b.clone()]);
        let frame = extract_frame(
            &scene,
            &sim_frame(2, Vec::new(), Vec::new()),
            &previous,
            RenderQuality::Preview,
        )
        .unwrap();
        assert!(frame.cut, "camera identity changed → cut");
        assert_eq!(frame.camera_id, Some(cam_b.id));

        // Same camera → no cut
        let scene_same = snapshot(Vec::new(), vec![cam_a.clone()]);
        let no_cut = extract_frame(
            &scene_same,
            &sim_frame(3, Vec::new(), Vec::new()),
            &previous,
            RenderQuality::Preview,
        )
        .unwrap();
        assert!(!no_cut.cut);
    }

    #[test]
    fn record_commits_camera_pair_and_cut_serial() {
        let cam = active_camera(NodeId::new_v4());
        let mut previous = PreviousFrameState::fresh();
        let scene = snapshot(Vec::new(), vec![cam.clone()]);
        // history invalid → cut false, but submission records the camera
        let frame = extract_frame(
            &scene,
            &sim_frame(1, Vec::new(), Vec::new()),
            &previous,
            RenderQuality::Preview,
        )
        .unwrap();
        let before_serial = previous.history.cut_serial;
        previous.record_successful_submission(&frame, 42);
        assert_eq!(previous.previous_camera_id, Some(cam.id));
        assert!(previous.history.valid);
        assert_eq!(previous.history.previous_tick, Some(SimTick(1)));
        assert_eq!(previous.history.target_generation, 42);
        assert_eq!(previous.history.cut_serial, before_serial, "no cut → serial unchanged");

        // camera SWITCH next frame → cut detected + serial bumps on record
        let cam2 = active_camera(NodeId::new_v4());
        let scene2 = snapshot(Vec::new(), vec![cam2.clone()]);
        let frame2 = extract_frame(
            &scene2,
            &sim_frame(2, Vec::new(), Vec::new()),
            &previous,
            RenderQuality::Preview,
        )
        .unwrap();
        assert!(frame2.cut);
        previous.record_successful_submission(&frame2, 43);
        assert_eq!(previous.history.cut_serial, before_serial + 1);
        assert_eq!(previous.previous_camera_id, Some(cam2.id));
    }
}
