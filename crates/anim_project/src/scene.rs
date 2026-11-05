//! Raw scenes, nodes, tracks and markers (spec 12.3).
//!
//! These are AUTHORITATIVE data (not evaluated snapshots — WP04 produces
//! those and reads them read-only). Structure is plain data with invariants
//! enforced by schema validation + commands:
//!   - nodes are parent-linked within their scene; cycles impossible
//!   - an asset reference must exist in the WP02 manifest
//!   - tracks target existing nodes

use crate::property::PropertyTrack;
use crate::ProjectError;
use anim_core::{AssetId, FramePos, NodeId, TrackId};
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
}
