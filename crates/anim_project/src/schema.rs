//! Validation and canonical serialization (spec 12.3, 12.5 step 1).
//!
//! Canonical bytes: `serde_json::to_vec` over a struct whose every map is a
//! BTreeMap keyed by byte-order IDs and whose Vectors carry only persisted
//! (deterministic) order. No timestamps inside the document — the document
//! hash is then reproducible across processes (G3-style requirement).

use crate::document::{check_schema_version, ProjectDocument};
use crate::ProjectError;
use std::path::Path;

/// Full document validation. Assumes callers may have touched fields
/// directly (serde construction); checks EVERY documented invariant.
pub fn validate_document(doc: &ProjectDocument) -> Result<(), ProjectError> {
    check_schema_version(doc)?;

    // frame_rate fields are pub (WP01) — re-validate here.
    let rate = anim_core::FrameRate::new(doc.frame_rate.num, doc.frame_rate.den)?;
    let _ = rate;

    // Simulation settings invariants (WP05 §14.2).
    doc.simulation
        .validate()
        .map_err(|e| ProjectError::Validation(format!("simulation settings: {e}")))?;

    // Manifest entry invariants (Deserialize bypasses the checked ctor).
    for entry in doc.assets.records.values() {
        entry.validate().map_err(|e| ProjectError::Validation(format!("manifest: {e}")))?;
    }

    if doc.project_id.0.is_nil() {
        return Err(ProjectError::Validation("project id must not be nil".into()));
    }

    for (scene_id, scene) in &doc.scenes {
        if scene_id.0.is_nil() {
            return Err(ProjectError::Validation("scene id must not be nil".into()));
        }
        scene.validate_node_parents()?;
        scene.validate_event_definitions(&doc.assets)?;
        scene.validate_simulation_inputs()?;
        for (node_id, node) in &scene.nodes {
            if node_id.0.is_nil() {
                return Err(ProjectError::Validation("node id must not be nil".into()));
            }
            if let Some(asset) = node.asset {
                if doc.assets.get(&asset).is_none() {
                    return Err(ProjectError::Validation(format!(
                        "node {node_id}: asset {asset} not present in manifest"
                    )));
                }
            }
            for (name, track) in &node.properties {
                if name.is_empty() {
                    return Err(ProjectError::Validation(
                        "property name must not be empty".into(),
                    ));
                }
                track.validate()?;
            }
        }
        for (track_id, track) in &scene.tracks {
            if track_id.0.is_nil() {
                return Err(ProjectError::Validation("track id must not be nil".into()));
            }
            if !scene.nodes.contains_key(&track.target) {
                return Err(ProjectError::Validation(format!(
                    "track {track_id}: target node missing from scene"
                )));
            }
            if track.property.is_empty() {
                return Err(ProjectError::Validation(
                    "track property must not be empty".into(),
                ));
            }
        }
    }
    Ok(())
}

/// Deterministic canonical serialization (serde_json compact; BTreeMaps are
/// serialization-ordered; Vecs are persisted order).
pub fn canonical_json(doc: &ProjectDocument) -> Result<Vec<u8>, ProjectError> {
    // validate first — never serialize an invalid document (spec 12.7
    // "Validate and serialize before touching disk").
    validate_document(doc)?;
    let bytes = serde_json::to_vec(doc)
        .map_err(|e| ProjectError::Serialize(format!("canonical serialization: {e}")))?;
    // Canonical JSON: floats are finite (validation guarantees), BTreeMaps
    // serialize in key order.
    Ok(bytes)
}

/// Parse + full-validate a document from bytes (the load path).
pub fn parse_document(bytes: &[u8]) -> Result<ProjectDocument, ProjectError> {
    let doc: ProjectDocument = serde_json::from_slice(bytes)
        .map_err(|e| ProjectError::Validation(format!("parse: {e}")))?;
    validate_document(&doc)?;
    Ok(doc)
}

/// Load a document file (the reopen path in the Phase 1 exit demo).
pub fn load_document(destination: &Path) -> Result<ProjectDocument, ProjectError> {
    let bytes =
        std::fs::read(destination).map_err(|e| ProjectError::Io(format!("read: {e}")))?;
    parse_document(&bytes)
}

/// Prove a document's canonical bytes hash to the SAME value in a fresh
/// process — used by the Phase 1 exit demo (save → hash → reopen → hash).
pub fn document_hash(
    doc: &ProjectDocument,
) -> Result<anim_assets::ContentHash, ProjectError> {
    let bytes = canonical_json(doc)?;
    let mut hasher = anim_assets::hash::StreamingHasher::new();
    for chunk in bytes.chunks(anim_assets::store::STREAM_CHUNK) {
        hasher.update(chunk);
    }
    Ok(hasher.finalize())
}
