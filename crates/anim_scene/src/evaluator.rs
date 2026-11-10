//! Stateless evaluation entry point (spec §13.9 interfaces).
//!
//! `SceneEvaluator::evaluate/&evaluate_frame` NEVER touch caches and never
//! accumulate time from previous frames; the same (document revision,
//! scene, position) always yields the SAME snapshot. Caching (§13.8) is a
//! separate, explicit `SceneCache` layer above these functions.
//!
//! Evaluation order per §13.5:
//!   1. validate hierarchy/property keys/tracks/event definitions
//!   2. evaluate local properties at the exact FramePos time
//!   3. blend animation layers (authored priority override — ADR-0008)
//!   4. world transforms in validated topological tree order
//!   5. poses, visibility, cameras and audio cues
//!   6. events owned by the frame interval + canonical occurrence ids
//!   7. freeze into Arc slices + canonical hash availability

use crate::animation::{basic_pose, layer_override_value, BasicPose};
use crate::camera::evaluate_cameras;
use crate::error::{SceneError, SceneResult};
use crate::events::{events_for_frame_impl, occurrences_in_frame};
use crate::hierarchy::{
    frame_window, local_pose, time_of, traverse, validate_scene, LocalPose,
};
use crate::snapshot::{
    snapshot_hash, AudioCue, EvaluatedNode, SceneEvent, SceneSnapshot, SimulationInput,
};
use anim_core::{FramePos, Rational, SceneId};
use anim_project::{EventKind, ProjectDocument};
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct SceneEvaluator;

impl SceneEvaluator {
    /// Evaluate a scene at an ARBITRARY frame position (whole or subframe).
    pub fn evaluate(
        document: &ProjectDocument,
        scene_id: SceneId,
        position: FramePos,
    ) -> SceneResult<Arc<SceneSnapshot>> {
        let scene = document
            .scenes
            .get(&scene_id)
            .ok_or_else(|| SceneError::MissingScene(scene_id.to_string()))?;

        // 1. full validation (cycle/parent/dup/props/defs + camera cuts) BEFORE
        //    ANY evaluation (spec §13.5 step 1, §13.11 typed rejection)
        validate_scene(scene)?;
        validate_event_defs(document, scene)?;
        crate::camera::validate_camera_roles(scene)?;

        // exact time of THIS position (never accumulated across frames)
        let t = time_of(position.clone(), document.frame_rate)?;
        // the WHOLE-frame window this position lies in (event ownership);
        // borrowed sentinel keeps the bindings alive in release builds
        let (window_start, window_end) =
            frame_window(document.frame_rate, position.frame())?;
        let _ = (&window_start, &window_end);

        // 2+3. every node's local properties, with layer overrides applied
        // 4. world = parent_world * local in validated traversal order
        let order = traverse(scene);
        let mut worlds: BTreeMap<anim_core::NodeId, crate::hierarchy::Mat3> =
            BTreeMap::new();
        let mut nodes_out: Vec<EvaluatedNode> = Vec::with_capacity(order.len());
        let mut poses: BTreeMap<anim_core::NodeId, BasicPose> = BTreeMap::new();

        for id in order {
            let node = scene.nodes.get(&id).expect("traversal proved existence");
            let pose: LocalPose = local_pose(node, &t)?;
            let local = pose.transform;
            let world = match node.parent {
                Some(parent) => {
                    let parent_world = worlds[&parent];
                    crate::hierarchy::mat_mul(&parent_world, &local)
                }
                None => local,
            };
            crate::hierarchy::require_finite_mat(&world, &node.name, "world")?;
            worlds.insert(id, world);
            poses.insert(id, basic_pose(node, &t)?);

            // 3. layer override: a track with the highest layer may REPLACE
            //    z/opacity/visible at this position (ADR-0008 MVP blend)
            let z = override_or(scene, id, "z", pose.z, &t)?;
            let opacity =
                override_or(scene, id, "opacity", pose.opacity, &t)?.clamp(0.0, 1.0);
            let visible = override_bool_or(scene, id, "visible", pose.visible, &t)?;

            nodes_out.push(EvaluatedNode {
                id,
                parent: node.parent,
                world_transform: world,
                z,
                opacity,
                visible,
                asset: node.asset,
            });
        }

        // 5. cameras (cut-resolved) and audio cues + sim inputs from events
        let cameras = evaluate_cameras(scene, position.clone(), &t)?;

        // 6. events OWNED by the whole frame n = [time_of(n), time_of(n+1))
        //    (spec §13.6 ownership rule; subframe positions do NOT redefine
        //    delivery — simulation uses the canonical event sequence)
        let event_vec = occurrences_in_frame(scene, document.frame_rate, position.frame())?;
        let mut audio_cues: Vec<AudioCue> = Vec::new();
        let mut simulation_inputs: Vec<SimulationInput> = Vec::new();
        for (def_id, def) in &scene.events {
            for (idx, time) in def.times.iter().enumerate() {
                let usize_idx = u32::try_from(idx).map_err(|_| {
                    SceneError::EventOrderOverflow(format!("definition {def_id}"))
                })?;
                let containing = document.frame_rate.frame_containing(time.clone())?;
                if containing.frame() != position.frame() {
                    continue;
                }
                // exact-time membership asserted against the window (defense
                // in depth for the ownership rule)
                let time_exact = document.frame_rate.time_of(containing.clone())?;
                debug_assert!(time_exact >= window_start && time_exact < window_end);

                match def.kind {
                    EventKind::Cue => audio_cues.push(AudioCue {
                        definition: *def_id,
                        occurrence: crate::events::occurrence_id(def_id, usize_idx),
                        name: def.name.clone(),
                        asset: def.asset,
                        at: containing,
                    }),
                    EventKind::Spawn => simulation_inputs.push(SimulationInput {
                        occurrence: crate::events::occurrence_id(def_id, usize_idx),
                        source: def.source,
                        local_sequence: usize_idx,
                        at: time_exact_param(time)?,
                        params: def.params.clone(),
                    }),
                    EventKind::Note => {}
                }
            }
        }
        // canonical order for the derived slices too (size-key framing)
        audio_cues.sort_by(|a, b| a.at.cmp(&b.at).then(a.occurrence.cmp(&b.occurrence)));
        simulation_inputs
            .sort_by(|a, b| a.at.cmp(&b.at).then(a.occurrence.cmp(&b.occurrence)));

        let _: Vec<SceneEvent> = Vec::new(); // silence; events vec already built

        let snapshot = SceneSnapshot {
            scene_id,
            revision: document.revision,
            position,
            time: t,
            nodes: nodes_out.into(),
            cameras: cameras.into(),
            audio_cues: audio_cues.into(),
            simulation_inputs: simulation_inputs.into(),
            events: event_vec.into(),
        };

        // hashes must always be computable on valid snapshots (finite floats
        // guaranteed by construction) — prove here, not lazily later
        snapshot_hash(&snapshot)?;

        Ok(Arc::new(snapshot))
    }

    /// Evaluate a WHOLE frame (position = frame start, subframe 0).
    pub fn evaluate_frame(
        document: &ProjectDocument,
        scene_id: SceneId,
        frame: i64,
    ) -> SceneResult<Arc<SceneSnapshot>> {
        let position = FramePos::new(frame, Rational::zero())?;
        Self::evaluate(document, scene_id, position)
    }
}

/// The events OWNED by whole frame n (spec §13.9): [time_of(n), time_of(n+1)).
/// An event exactly at the upper boundary belongs to the NEXT frame.
pub fn events_for_frame(
    document: &ProjectDocument,
    scene_id: SceneId,
    frame: i64,
) -> SceneResult<Arc<[SceneEvent]>> {
    events_for_frame_impl(document, scene_id, frame).map(|v| v.into())
}

/// Layer override plumbing shared by z/opacity/visible reads.
fn override_or(
    scene: &anim_project::SceneData,
    id: anim_core::NodeId,
    property: &str,
    fallback: f64,
    t: &Rational,
) -> SceneResult<f64> {
    match layer_override_value(scene, id, property, t)? {
        Some(anim_project::PropertyValue::Float(f)) if f.is_finite() => Ok(f),
        Some(anim_project::PropertyValue::Float(f)) => {
            Err(SceneError::InvalidNumericValue(format!(
                "layer override '{property}' nonfinite: {f}"
            )))
        }
        Some(other) => Err(SceneError::InvalidPropertyKeys(format!(
            "layer override '{property}': expected Float, found {other:?}"
        ))),
        None => Ok(fallback),
    }
}

fn override_bool_or(
    scene: &anim_project::SceneData,
    id: anim_core::NodeId,
    property: &str,
    fallback: bool,
    t: &Rational,
) -> SceneResult<bool> {
    match layer_override_value(scene, id, property, t)? {
        Some(anim_project::PropertyValue::Bool(b)) => Ok(b),
        Some(other) => Err(SceneError::InvalidPropertyKeys(format!(
            "layer override '{property}': expected Bool, found {other:?}"
        ))),
        None => Ok(fallback),
    }
}

/// Rational param passthrough with overflow typing.
fn time_exact_param(t: &Rational) -> SceneResult<Rational> {
    Ok(t.clone())
}

/// Event-definition validation on the evaluation path (typed, per §13.11).
fn validate_event_defs(
    document: &ProjectDocument,
    scene: &anim_project::SceneData,
) -> SceneResult<()> {
    scene.validate_event_definitions(&document.assets).map_err(|e| match e {
        anim_project::ProjectError::Validation(msg) => SceneError::InvalidPropertyKeys(msg),
        other => SceneError::TimeOverflow(other.to_string()),
    })
}
