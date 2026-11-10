//! Raw scenes, nodes, tracks and markers (spec 12.3).
//!
//! These are AUTHORITATIVE data (not evaluated snapshots — WP04 produces
//! those and reads them read-only). Structure is plain data with invariants
//! enforced by schema validation + commands:
//!   - nodes are parent-linked within their scene; cycles impossible
//!   - an asset reference must exist in the WP02 manifest
//!   - tracks target existing nodes
//!   - event definitions reference existing nodes and carry strictly
//!     increasing occurrence times (WP04 §13 prerequisite)

use crate::property::{PropertyTrack, PropertyValue};
use crate::{ProjectError, ProjectResult};
use anim_core::{
    AssetId, EmitterSettings, EventDefinitionId, FramePos, NodeId, Rational, SpringEdge,
    SpringEdgeId, TrackId,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SceneData {
    pub name: String,
    /// Ordered by NodeId byte order (BTreeMap) — canonical serialization
    /// never depends on insertion order.
    pub nodes: BTreeMap<NodeId, NodeData>,
    pub tracks: BTreeMap<TrackId, TrackData>,
    pub markers: Vec<SceneMarker>,
    /// Event definitions per spec §13 — WP04 derives concrete occurrences
    /// from these at evaluation time. Ordered by EventDefinitionId byte
    /// order; `serde(default)` keeps WP03-era documents loadable.
    #[serde(default)]
    pub events: BTreeMap<EventDefinitionId, EventDefinition>,
    /// Stochastic emitters per spec §14 (WP05): node → settings. Ordered by
    /// NodeId; `serde(default)` keeps WP04-era documents loadable.
    #[serde(default)]
    pub emitters: BTreeMap<NodeId, EmitterSettings>,
    /// Spring edges between scene nodes (WP05). Ordered by SpringEdgeId;
    /// `serde(default)` keeps former documents loadable.
    #[serde(default)]
    pub springs: BTreeMap<SpringEdgeId, SpringEdge>,
}

/// What an event occurrence MEANS downstream (kept in the authoritative
/// document; anim_scene re-exports it — dependency direction forbids the
/// reverse). MVP taxonomy matches proved consumer needs: emitter spawns,
/// audio cues and inspection notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    /// Animation-layer emission event (WP05 compiles it to a tick).
    Spawn,
    /// Audio cue with an optional bound asset (WP11 converts to samples).
    Cue,
    /// Editorial/annotation event with no simulation side effect.
    Note,
}

/// One schedulable event definition: WHO fires it, at WHAT exact times,
/// with WHAT priority and optional parameters (WP04 §13).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventDefinition {
    pub name: String,
    pub kind: EventKind,
    /// Sort priority in the canonical event key.
    pub priority: i32,
    /// The node producing occurrences (must exist in the scene).
    pub source: NodeId,
    /// Optional audio asset binding (Cue events; must exist in manifest).
    #[serde(default)]
    pub asset: Option<AssetId>,
    /// EXACT occurrence times, strictly increasing (validation invariant;
    /// the index in this vec IS the occurrence_index — a stable source key,
    /// never enumeration order per §13.7).
    pub times: Vec<Rational>,
    /// Named parameters carried into the occurrence payload.
    #[serde(default)]
    pub params: BTreeMap<String, PropertyValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeData {
    pub name: String,
    /// Parent within the SAME scene; None = root-level.
    pub parent: Option<NodeId>,
    /// Optional immutable asset reference (must exist in the manifest).
    pub asset: Option<AssetId>,
    /// Per-property key tracks keyed by canonical property name.
    pub properties: BTreeMap<String, PropertyTrack>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackData {
    pub name: String,
    /// The animated node this track drives.
    pub target: NodeId,
    /// Blend priority layer (WP04: layers blend in authored priority order).
    pub layer: u32,
    /// The node property this track writes (must be nonempty).
    pub property: String,
}

/// A named marker in the scene timeline (camera cut or an editorial note).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneMarker {
    pub name: String,
    pub at: FramePos,
    pub kind: MarkerKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarkerKind {
    Cut,
    Note,
}

impl SceneData {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            nodes: BTreeMap::new(),
            tracks: BTreeMap::new(),
            markers: Vec::new(),
            events: BTreeMap::new(),
            emitters: BTreeMap::new(),
            springs: BTreeMap::new(),
        }
    }

    /// Ancestors of `node` following persisted parent refs; empty root set.
    pub(crate) fn ancestors(&self, node: NodeId) -> Vec<NodeId> {
        let mut chain = Vec::new();
        let mut cursor = self.nodes.get(&node).and_then(|n| n.parent);
        while let Some(p) = cursor {
            if chain.contains(&p) {
                break; // validator guarantees this can't happen; belt + braces
            }
            chain.push(p);
            cursor = self.nodes.get(&p).and_then(|n| n.parent);
        }
        chain
    }

    pub(crate) fn validate_node_parents(&self) -> Result<(), ProjectError> {
        for (id, node) in &self.nodes {
            if let Some(p) = node.parent {
                if !self.nodes.contains_key(&p) {
                    return Err(ProjectError::Validation(format!(
                        "node {id}: parent {p} missing from scene"
                    )));
                }
                // cycle = self in own ancestor chain
                if self.ancestors(*id).contains(id) {
                    return Err(ProjectError::Validation(format!(
                        "node {id}: parent cycle detected"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Simulation authoring invariants (WP05 §14.2): emitter/spring nodes
    /// exist in the scene, endpoints differ, and every numeric setting is
    /// finite/in-range (delegated to the anim_core validators).
    pub(crate) fn validate_simulation_inputs(&self) -> Result<(), ProjectError> {
        for (node, settings) in &self.emitters {
            if !self.nodes.contains_key(node) {
                return Err(ProjectError::Validation(format!(
                    "emitter: node {node} missing from scene"
                )));
            }
            settings
                .validate()
                .map_err(|e| ProjectError::Validation(format!("emitter {node}: {e}")))?;
        }
        for (id, edge) in &self.springs {
            if !self.nodes.contains_key(&edge.a) {
                return Err(ProjectError::Validation(format!(
                    "spring edge {id}: endpoint A {} missing from scene",
                    edge.a
                )));
            }
            if !self.nodes.contains_key(&edge.b) {
                return Err(ProjectError::Validation(format!(
                    "spring edge {id}: endpoint B {} missing from scene",
                    edge.b
                )));
            }
            if edge.a == edge.b {
                return Err(ProjectError::Validation(format!(
                    "spring edge {id}: endpoints A and B are the same node"
                )));
            }
            edge.validate()
                .map_err(|e| ProjectError::Validation(format!("spring edge {id}: {e}")))?;
        }
        Ok(())
    }

    /// Event definition invariants (WP04 §13.5 step 1): source node exists,
    /// cue assets exist in the manifest, occurrence times strictly
    /// increasing, params finite. Public: the evaluator crate re-validates
    /// on the evaluation path (spec §13.11 — never evaluate a malformed
    /// definition silently).
    pub fn validate_event_definitions(
        &self,
        manifest: &anim_assets::AssetManifest,
    ) -> ProjectResult<()> {
        for (id, def) in &self.events {
            if !self.nodes.contains_key(&def.source) {
                return Err(ProjectError::Validation(format!(
                    "event definition {id}: source node {} missing from scene",
                    def.source
                )));
            }
            if let Some(a) = def.asset {
                if manifest.get(&a).is_none() {
                    return Err(ProjectError::Validation(format!(
                        "event definition {id}: asset {a} not present in manifest"
                    )));
                }
            }
            for w in def.times.windows(2) {
                if w[0] >= w[1] {
                    return Err(ProjectError::Validation(format!(
                        "event definition {id}: occurrence times not strictly increasing"
                    )));
                }
            }
            for (name, value) in &def.params {
                if name.is_empty() {
                    return Err(ProjectError::Validation(format!(
                        "event definition {id}: empty parameter name"
                    )));
                }
                value
                    .validate()
                    .map_err(|e| ProjectError::Validation(format!("event {id}: {e}")))?;
            }
        }
        Ok(())
    }
}
