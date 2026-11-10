//! Immutable snapshot and evaluated-node types + canonical hash (spec
//! §13.8). Snapshots expose NO setters and NO interior mutability; every
//! collection is frozen into an `Arc<[_]>` slice.
//!
//! `snapshot_hash` is G3-relevant: fixed-width little-endian encoding in
//! declaration order, f64 encoded as BIT PATTERNS after finite validation,
//! domain-separated ("anim_scene_snapshot_v1"), so two fresh processes
//! hashing the same snapshot MUST agree byte for byte.

use crate::error::{SceneError, SceneResult};
use crate::hierarchy::Mat3;
use anim_assets::ContentHash;
use anim_core::{
    domain_hash, AssetId, EventDefinitionId, EventOccurrenceId, FramePos, NodeId, Rational,
    Revision, SceneId,
};
use anim_project::{MarkerKind, PropertyValue, SceneMarker};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Canonical camera model (ADR-0008): any node flagged as a camera;
/// `active` resolved from Cut markers at the evaluated position.
#[derive(Debug, Clone, PartialEq)]
pub struct EvaluatedCamera {
    pub id: NodeId,
    pub center_x: f64,
    pub center_y: f64,
    pub rotation: f64,
    pub zoom: f64,
    /// The shot's selected camera (Cut marker rule, camera.rs).
    pub active: bool,
}

/// One audio cue occurrence with its optional asset binding (converted to
/// exact sample positions by anim_audio, never by this crate — §13.10).
#[derive(Debug, Clone, PartialEq)]
pub struct AudioCue {
    pub definition: EventDefinitionId,
    pub occurrence: EventOccurrenceId,
    pub name: String,
    pub asset: Option<AssetId>,
    pub at: FramePos,
}

/// One simulation-relevant input (Spawn-kind events, §13.10: anim_sim
/// consumes these schedule inputs read-only).
#[derive(Debug, Clone, PartialEq)]
pub struct SimulationInput {
    pub occurrence: EventOccurrenceId,
    pub source: NodeId,
    pub local_sequence: u32,
    /// Exact spawn time (rational seconds).
    pub at: Rational,
    pub params: BTreeMap<String, PropertyValue>,
}

/// One canonical event occurrence (spec §13.8 verbatim fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneEvent {
    pub occurrence_id: EventOccurrenceId,
    pub time: FramePos,
    pub priority: i32,
    pub source_id: NodeId,
    pub local_sequence: u32,
    pub kind: anim_project::EventKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvaluatedNode {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub world_transform: Mat3,
    pub z: f64,
    pub opacity: f64,
    pub visible: bool,
    pub asset: Option<AssetId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SceneSnapshot {
    pub scene_id: SceneId,
    pub revision: Revision,
    pub position: FramePos,
    /// Exact rational time of the WHOLE position: t = time_of(position).
    pub time: Rational,
    /// Hierarchy traversal order (parent-first, NodeId-ordered siblings).
    pub nodes: Arc<[EvaluatedNode]>,
    pub cameras: Arc<[EvaluatedCamera]>,
    pub audio_cues: Arc<[AudioCue]>,
    pub simulation_inputs: Arc<[SimulationInput]>,
    /// The events OWNED by this whole frame (see §13.6 ownership rule).
    pub events: Arc<[SceneEvent]>,
}

/// Cut markers of a scene, sorted by time (used by camera.rs + hashing of
/// the marker payload is NOT part of the snapshot — cameras already carry
/// the resolved cut state).
pub(crate) fn cut_markers(markers: &[SceneMarker]) -> Vec<&SceneMarker> {
    let mut cuts: Vec<&SceneMarker> =
        markers.iter().filter(|m| m.kind == MarkerKind::Cut).collect();
    cuts.sort_by(|a, b| a.at.cmp(&b.at));
    cuts
}

/// Draw extraction keys produced SEPARATELY from hierarchy order (spec
/// §13.7): ascending (z, NodeId). Invisible nodes are NOT skipped here —
/// consumers see the full extraction and cull via `visible`.
pub fn draw_order(snapshot: &SceneSnapshot) -> Vec<NodeId> {
    let mut keyed: Vec<(u64, NodeId)> =
        snapshot.nodes.iter().map(|n| (n.z.to_bits(), n.id)).collect();
    keyed.sort();
    keyed.into_iter().map(|(_, id)| id).collect()
}

// ------------------------------------------------------------------ hash

/// Canonical event-kind tag (hash stability: pinned, never reordered).
fn event_kind_tag(kind: anim_project::EventKind) -> u8 {
    match kind {
        anim_project::EventKind::Spawn => 0,
        anim_project::EventKind::Cue => 1,
        anim_project::EventKind::Note => 2,
    }
}
const ZERO_UUID: [u8; 16] = [0u8; 16];

fn append_f64(out: &mut Vec<u8>, v: f64) -> SceneResult<()> {
    if !v.is_finite() {
        return Err(SceneError::InvalidNumericValue(
            "nonfinite value cannot be canonically hashed".into(),
        ));
    }
    out.extend_from_slice(&v.to_bits().to_le_bytes());
    Ok(())
}

fn append_value(out: &mut Vec<u8>, v: &PropertyValue) -> SceneResult<()> {
    match v {
        PropertyValue::Float(f) => {
            out.push(0);
            append_f64(out, *f)?;
        }
        PropertyValue::Int(i) => {
            out.push(1);
            out.extend_from_slice(&i.to_le_bytes());
        }
        PropertyValue::Bool(b) => {
            out.push(2);
            out.push(u8::from(*b));
        }
        PropertyValue::Text(s) => {
            out.push(3);
            out.extend_from_slice(&(s.len() as u64).to_le_bytes());
            out.extend_from_slice(s.as_bytes());
        }
    }
    Ok(())
}

fn append_framepos(out: &mut Vec<u8>, p: &FramePos) {
    out.extend_from_slice(&p.frame().to_le_bytes());
    out.extend_from_slice(&p.subframe().num().to_le_bytes());
    out.extend_from_slice(&p.subframe().den().to_le_bytes());
}

fn append_optional_id(out: &mut Vec<u8>, id: &Option<NodeId>) {
    match id {
        Some(id) => out.extend_from_slice(id.0.as_bytes()),
        None => out.extend_from_slice(&ZERO_UUID),
    }
}

fn append_matrix(out: &mut Vec<u8>, m: &Mat3) -> SceneResult<()> {
    for row in m {
        for v in row {
            append_f64(out, *v)?;
        }
    }
    Ok(())
}

fn append_node(out: &mut Vec<u8>, n: &EvaluatedNode) -> SceneResult<()> {
    out.extend_from_slice(n.id.0.as_bytes());
    append_optional_id(out, &n.parent);
    append_matrix(out, &n.world_transform)?;
    append_f64(out, n.z)?;
    append_f64(out, n.opacity)?;
    out.push(u8::from(n.visible));
    match n.asset {
        Some(a) => out.extend_from_slice(a.0.as_bytes()),
        None => out.extend_from_slice(&ZERO_UUID),
    }
    Ok(())
}

fn append_camera(out: &mut Vec<u8>, c: &EvaluatedCamera) -> SceneResult<()> {
    out.extend_from_slice(c.id.0.as_bytes());
    append_f64(out, c.center_x)?;
    append_f64(out, c.center_y)?;
    append_f64(out, c.rotation)?;
    append_f64(out, c.zoom)?;
    out.push(u8::from(c.active));
    Ok(())
}

fn append_audio_cue(out: &mut Vec<u8>, c: &AudioCue) {
    out.extend_from_slice(c.definition.0.as_bytes());
    out.extend_from_slice(c.occurrence.0.as_bytes());
    out.extend_from_slice(&(c.name.len() as u64).to_le_bytes());
    out.extend_from_slice(c.name.as_bytes());
    match c.asset {
        Some(a) => out.extend_from_slice(a.0.as_bytes()),
        None => out.extend_from_slice(&ZERO_UUID),
    }
    append_framepos(out, &c.at);
}

fn append_sim_input(out: &mut Vec<u8>, i: &SimulationInput) -> SceneResult<()> {
    out.extend_from_slice(i.occurrence.0.as_bytes());
    out.extend_from_slice(i.source.0.as_bytes());
    out.extend_from_slice(&i.local_sequence.to_le_bytes());
    out.extend_from_slice(&i.at.num().to_le_bytes());
    out.extend_from_slice(&i.at.den().to_le_bytes());
    out.extend_from_slice(&(i.params.len() as u64).to_le_bytes());
    // BTreeMap → name AND value in key order (never enumeration order)
    for (name, value) in &i.params {
        out.extend_from_slice(&(name.len() as u64).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        append_value(out, value)?;
    }
    Ok(())
}

fn append_event(out: &mut Vec<u8>, e: &SceneEvent) {
    out.extend_from_slice(e.occurrence_id.0.as_bytes());
    append_framepos(out, &e.time);
    out.extend_from_slice(&e.priority.to_le_bytes());
    out.extend_from_slice(e.source_id.0.as_bytes());
    out.extend_from_slice(&e.local_sequence.to_le_bytes());
    out.push(event_kind_tag(e.kind));
}

/// Canonical snapshot hash (spec §13.9 interface). Errors on ANY nonfinite
/// float (snapshot construction already rejects them; this is the belt).
pub fn snapshot_hash(snapshot: &SceneSnapshot) -> SceneResult<ContentHash> {
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(snapshot.scene_id.0.as_bytes());
    out.extend_from_slice(&snapshot.revision.as_u64().to_le_bytes());
    append_framepos(&mut out, &snapshot.position);
    out.extend_from_slice(&snapshot.time.num().to_le_bytes());
    out.extend_from_slice(&snapshot.time.den().to_le_bytes());

    out.extend_from_slice(&(snapshot.nodes.len() as u64).to_le_bytes());
    for n in snapshot.nodes.iter() {
        append_node(&mut out, n)?;
    }

    out.extend_from_slice(&(snapshot.cameras.len() as u64).to_le_bytes());
    for c in snapshot.cameras.iter() {
        append_camera(&mut out, c)?;
    }

    out.extend_from_slice(&(snapshot.audio_cues.len() as u64).to_le_bytes());
    for c in snapshot.audio_cues.iter() {
        append_audio_cue(&mut out, c);
    }

    out.extend_from_slice(&(snapshot.simulation_inputs.len() as u64).to_le_bytes());
    for i in snapshot.simulation_inputs.iter() {
        append_sim_input(&mut out, i)?;
    }

    out.extend_from_slice(&(snapshot.events.len() as u64).to_le_bytes());
    for e in snapshot.events.iter() {
        append_event(&mut out, e);
    }

    Ok(ContentHash(domain_hash("anim_scene_snapshot_v1", &out)))
}

/// Canonical EVENT-LOG digest over an ordered slice of occurrences (used
/// by the G3 two-process proof and checkpoint cross-references: WP05
/// compares event logs, not whole snapshots).
pub fn events_hash(events: &[SceneEvent]) -> SceneResult<ContentHash> {
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(&(events.len() as u64).to_le_bytes());
    for e in events {
        append_event(&mut out, e);
    }
    Ok(ContentHash(domain_hash("anim_scene_events_v1", &out)))
}
