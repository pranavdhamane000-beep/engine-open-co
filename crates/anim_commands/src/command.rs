//! Commands with validation, forward mutation and EXACT inverse generation
//! (spec 12.3, 12.5 step 2, 12.9).
//!
//! Contract of `Command::apply`:
//!   * validates against CURRENT transactional state (the processor is a
//!     single writer; commands see a cloned document mid-batch)
//!   * mutates forward
//!   * returns the inverse command that restores the pre-state EXACTLY
//!   * any error leaves the document UNCHANGED (the processor drops the
//!     whole clone + inverses — no batch prefix ever commits, spec 12.11)

use crate::CommandError;
use anim_assets::AssetManifestEntry;
use anim_core::{AssetId, NodeId, SceneId};
use anim_project::{NodeData, ProjectDocument, PropertyKey, SceneData};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    /// Create an asset record (id must not exist; content from WP02 store).
    UpsertAsset {
        entry: AssetManifestEntry,
    },
    /// Re-import at the same identity with new content (new manifest
    /// revision of that asset; spec 11.6).
    ReplaceAsset {
        entry: AssetManifestEntry,
    },
    RemoveAsset {
        asset_id: AssetId,
    },
    /// Create when missing; renaming when present.
    UpsertScene {
        scene_id: SceneId,
        name: String,
    },
    RenameScene {
        scene_id: SceneId,
        name: String,
    },
    RemoveScene {
        scene_id: SceneId,
    },
    /// Create or update a node (name/parent/asset fields).
    UpsertNode {
        scene_id: SceneId,
        node_id: NodeId,
        name: String,
        parent: Option<NodeId>,
        asset: Option<AssetId>,
    },
    RemoveNode {
        scene_id: SceneId,
        node_id: NodeId,
    },
    /// Exact key upsert (replace-at-time); inverse restores the old key.
    SetNodeProperty {
        scene_id: SceneId,
        node_id: NodeId,
        property: String,
        key: PropertyKey,
    },
    RemoveNodePropertyKey {
        scene_id: SceneId,
        node_id: NodeId,
        property: String,
        key_time: anim_core::Rational,
    },
    SetFrameRate {
        num: u32,
        den: u32,
    },
    /// Restore a deleted scene verbatim (internal; the inverse of
    /// RemoveScene — serializable so undo re-materializes through the SAME
    /// command path, never a back door).
    RestoreSceneData {
        scene_id: SceneId,
        data: SceneData,
    },
    /// Restore a deleted node verbatim (the inverse of RemoveNode).
    RestoreNodeData {
        scene_id: SceneId,
        node_id: NodeId,
        data: NodeData,
    },
}

impl Command {
    pub fn type_name(&self) -> &'static str {
        match self {
            Command::UpsertAsset { .. } => "upsert_asset",
            Command::ReplaceAsset { .. } => "replace_asset",
            Command::RemoveAsset { .. } => "remove_asset",
            Command::UpsertScene { .. } => "upsert_scene",
            Command::RenameScene { .. } => "rename_scene",
            Command::RemoveScene { .. } => "remove_scene",
            Command::UpsertNode { .. } => "upsert_node",
            Command::RemoveNode { .. } => "remove_node",
            Command::SetNodeProperty { .. } => "set_node_property",
            Command::RemoveNodePropertyKey { .. } => "remove_node_property_key",
            Command::SetFrameRate { .. } => "set_frame_rate",
            Command::RestoreSceneData { .. } => "restore_scene_data",
            Command::RestoreNodeData { .. } => "restore_node_data",
        }
    }

    /// Apply against the transactional document; returns the EXACT inverse.
    pub fn apply(&self, txn: &mut ProjectDocument) -> Result<Command, CommandError> {
        match self {
            Command::UpsertAsset { entry } => upsert_asset(txn, entry, false),
            Command::ReplaceAsset { entry } => upsert_asset(txn, entry, true),
            Command::RemoveAsset { asset_id } => remove_asset(txn, *asset_id),
            Command::UpsertScene { scene_id, name } => upsert_scene(txn, *scene_id, name),
            Command::RenameScene { scene_id, name } => rename_scene(txn, *scene_id, name),
            Command::RemoveScene { scene_id } => remove_scene(txn, *scene_id),
            Command::UpsertNode { scene_id, node_id, name, parent, asset } => {
                upsert_node(txn, *scene_id, *node_id, name, *parent, *asset)
            }
            Command::RemoveNode { scene_id, node_id } => {
                remove_node(txn, *scene_id, *node_id)
            }
            Command::SetNodeProperty { scene_id, node_id, property, key } => {
                set_node_property(txn, *scene_id, *node_id, property, key)
            }
            Command::RemoveNodePropertyKey { scene_id, node_id, property, key_time } => {
                remove_node_property_key(txn, *scene_id, *node_id, property, key_time)
            }
            Command::SetFrameRate { num, den } => set_frame_rate(txn, *num, *den),
            // Restore commands are the undo-side of removals: they re-insert
            // the stored verbatim data; their notional inverse (discarded
            // by undo/redo machinery) is the removal that created them.
            Command::RestoreSceneData { scene_id, data } => {
                if txn.scenes.contains_key(scene_id) {
                    return Err(CommandError::Validation(format!(
                        "scene {scene_id} already present — restore only re-creates absent scenes"
                    )));
                }
                txn.scenes.insert(*scene_id, data.clone());
                Ok(Command::RemoveScene { scene_id: *scene_id })
            }
            Command::RestoreNodeData { scene_id, node_id, data } => {
                if scene_mut(txn, *scene_id)?.nodes.contains_key(node_id) {
                    return Err(CommandError::Validation(format!(
                        "node {node_id} already present — restore only re-creates absent nodes"
                    )));
                }
                scene_mut(txn, *scene_id)?.nodes.insert(*node_id, data.clone());
                Ok(Command::RemoveNode { scene_id: *scene_id, node_id: *node_id })
            }
        }
    }
}

fn scene_mut(
    txn: &mut ProjectDocument,
    scene_id: SceneId,
) -> Result<&mut SceneData, CommandError> {
    txn.scenes
        .get_mut(&scene_id)
        .ok_or_else(|| CommandError::Validation(format!("scene {scene_id} not found")))
}

fn node_mut(
    txn: &mut ProjectDocument,
    scene_id: SceneId,
    node_id: NodeId,
) -> Result<&mut NodeData, CommandError> {
    scene_mut(txn, scene_id)?.nodes.get_mut(&node_id).ok_or_else(|| {
        CommandError::Validation(format!("node {node_id} not found in scene {scene_id}"))
    })
}

// ----------------------------------------------------------------- assets

fn upsert_asset(
    txn: &mut ProjectDocument,
    entry: &AssetManifestEntry,
    allow_replace: bool,
) -> Result<Command, CommandError> {
    // Rebuild through the VALIDATED constructor — deserialized payloads must
    // pass the same invariants (sorted unique deps, no self-cycle).
    let entry = AssetManifestEntry::new(
        entry.asset_id,
        entry.content_hash,
        entry.byte_length,
        entry.kind,
        entry.importer_version,
        entry.dependencies.clone(),
        entry.display_name.clone(),
    )
    .map_err(|e| CommandError::Validation(format!("asset entry: {e}")))?;

    let existing = txn.assets.get(&entry.asset_id).cloned();
    match (&existing, allow_replace) {
        (Some(prev), _)
            if prev.content_hash == entry.content_hash
                && prev.byte_length == entry.byte_length
                && prev.dependencies == entry.dependencies =>
        {
            // idempotent no-op; applying the inverse (same upsert) is also a
            // no-op — exact inverse semantics preserved trivially.
            Ok(Command::UpsertAsset { entry: prev.clone() })
        }
        (Some(_), false) => Err(CommandError::Validation(format!(
            "asset {} already exists with different content — use replace_asset",
            entry.asset_id
        ))),
        (None, true) => Err(CommandError::Validation(format!(
            "asset {} does not exist — replace_asset only replaces existing records",
            entry.asset_id
        ))),
        _ => {
            if allow_replace {
                txn.assets
                    .replace(entry.clone())
                    .map_err(|e| CommandError::Validation(format!("manifest: {e}")))?;
            } else {
                txn.assets
                    .insert(entry.clone())
                    .map_err(|e| CommandError::Validation(format!("manifest: {e}")))?;
            }
            // inverse: restore the previous revision (recreate = upsert when
            // there was none; replace when there was)
            Ok(match existing {
                Some(prev) => Command::ReplaceAsset { entry: prev },
                None => Command::UpsertAsset { entry },
            })
        }
    }
}

fn remove_asset(
    txn: &mut ProjectDocument,
    asset_id: AssetId,
) -> Result<Command, CommandError> {
    let entry =
        txn.assets.get(&asset_id).cloned().ok_or_else(|| {
            CommandError::Validation(format!("asset {asset_id} not found"))
        })?;
    // integrity: no scene node may reference it
    for (sid, scene) in &txn.scenes {
        for (nid, node) in &scene.nodes {
            if node.asset == Some(asset_id) {
                return Err(CommandError::Validation(format!(
                    "asset {asset_id} still referenced by node {nid} in scene {sid}"
                )));
            }
        }
    }
    txn.assets
        .records
        .remove(&asset_id)
        .ok_or_else(|| CommandError::Validation(format!("asset {asset_id} vanished")))?;
    Ok(Command::UpsertAsset { entry })
}

// ----------------------------------------------------------------- scenes

fn upsert_scene(
    txn: &mut ProjectDocument,
    scene_id: SceneId,
    name: &str,
) -> Result<Command, CommandError> {
    match txn.scenes.get_mut(&scene_id) {
        Some(existing) => {
            let old_name = existing.name.clone();
            existing.name = name.to_string();
            Ok(Command::RenameScene { scene_id, name: old_name })
        }
        None => {
            txn.scenes.insert(scene_id, SceneData::new(name));
            Ok(Command::RemoveScene { scene_id })
        }
    }
}

fn rename_scene(
    txn: &mut ProjectDocument,
    scene_id: SceneId,
    name: &str,
) -> Result<Command, CommandError> {
    let existing = scene_mut(txn, scene_id)?;
    let old = existing.name.clone();
    existing.name = name.to_string();
    Ok(Command::RenameScene { scene_id, name: old })
}

fn remove_scene(
    txn: &mut ProjectDocument,
    scene_id: SceneId,
) -> Result<Command, CommandError> {
    let data = txn
        .scenes
        .remove(&scene_id)
        .ok_or_else(|| CommandError::Validation(format!("scene {scene_id} not found")))?;
    Ok(Command::RestoreSceneData { scene_id, data })
}

// ------------------------------------------------------------------ nodes

fn upsert_node(
    txn: &mut ProjectDocument,
    scene_id: SceneId,
    node_id: NodeId,
    name: &str,
    parent: Option<NodeId>,
    asset: Option<AssetId>,
) -> Result<Command, CommandError> {
    // asset referential integrity (payload persisted BEFORE the manifest
    // command, spec 12.2; the manifest record must exist first)
    if let Some(a) = asset {
        if txn.assets.get(&a).is_none() {
            return Err(CommandError::Validation(format!(
                "asset {a} not in manifest — import must precede node reference"
            )));
        }
    }
    {
        let scene = scene_mut(txn, scene_id)?;
        if let Some(p) = parent {
            if !scene.nodes.contains_key(&p) {
                return Err(CommandError::Validation(format!(
                    "parent {p} not found in scene {scene_id}"
                )));
            }
            // cycle guard: node must never appear in its own ancestry
            let mut cursor = Some(p);
            while let Some(id) = cursor {
                if id == node_id {
                    return Err(CommandError::Validation(format!(
                        "parenting {node_id} under {p} would create a cycle"
                    )));
                }
                cursor = scene.nodes.get(&id).and_then(|n| n.parent);
            }
        }
    }
    // strip parent immutability by re-borrowing fresh (borrow checker path)
    let old = {
        let scene = scene_mut(txn, scene_id)?;
        scene.nodes.get(&node_id).cloned()
    };
    let data = NodeData {
        name: name.to_string(),
        parent,
        asset,
        properties: old.as_ref().map(|o| o.properties.clone()).unwrap_or_default(),
    };
    let scene = scene_mut(txn, scene_id)?;
    scene.nodes.insert(node_id, data);
    Ok(match old {
        Some(prev) => Command::UpsertNode {
            scene_id,
            node_id,
            name: prev.name,
            parent: prev.parent,
            asset: prev.asset,
        },
        None => Command::RemoveNode { scene_id, node_id },
    })
}

fn remove_node(
    txn: &mut ProjectDocument,
    scene_id: SceneId,
    node_id: NodeId,
) -> Result<Command, CommandError> {
    {
        let scene = scene_mut(txn, scene_id)?;
        if !scene.nodes.contains_key(&node_id) {
            return Err(CommandError::Validation(format!(
                "node {node_id} not found in scene {scene_id}"
            )));
        }
        // integrity: children would dangle
        for (other_id, other) in &scene.nodes {
            if other.parent == Some(node_id) {
                return Err(CommandError::Validation(format!(
                    "node {node_id} has child {other_id} — reparent or remove children first"
                )));
            }
        }
        // integrity: tracks targeting this node would dangle
        for (tid, track) in &scene.tracks {
            if track.target == node_id {
                return Err(CommandError::Validation(format!(
                    "track {tid} targets node {node_id} — remove the track first"
                )));
            }
        }
    }
    let data = scene_mut(txn, scene_id)?
        .nodes
        .remove(&node_id)
        .ok_or_else(|| CommandError::Validation("node vanished mid-command".into()))?;
    Ok(Command::RestoreNodeData { scene_id, node_id, data })
}

// ------------------------------------------------------------- properties

fn set_node_property(
    txn: &mut ProjectDocument,
    scene_id: SceneId,
    node_id: NodeId,
    property: &str,
    key: &PropertyKey,
) -> Result<Command, CommandError> {
    if property.is_empty() {
        return Err(CommandError::Validation("property name must not be empty".into()));
    }
    key.value.validate().map_err(|e| CommandError::Validation(e.to_string()))?;
    let node = node_mut(txn, scene_id, node_id)?;
    let track = node.properties.entry(property.to_string()).or_default();
    // capture the OLD key BEFORE the upsert replaces it at that exact time
    let old = track.key_at(&key.time).cloned();
    track.upsert_key(key.clone());
    Ok(match old {
        Some(prev) => Command::SetNodeProperty {
            scene_id,
            node_id,
            property: property.to_string(),
            key: prev,
        },
        None => Command::RemoveNodePropertyKey {
            scene_id,
            node_id,
            property: property.to_string(),
            key_time: key.time.clone(),
        },
    })
}

fn remove_node_property_key(
    txn: &mut ProjectDocument,
    scene_id: SceneId,
    node_id: NodeId,
    property: &str,
    key_time: &anim_core::Rational,
) -> Result<Command, CommandError> {
    let node = node_mut(txn, scene_id, node_id)?;
    let track = node.properties.get_mut(property).ok_or_else(|| {
        CommandError::Validation(format!("property {property} not found"))
    })?;
    let removed = track.remove_key(key_time).ok_or_else(|| {
        CommandError::Validation(format!("no key at {key_time} in property {property}"))
    })?;
    Ok(Command::SetNodeProperty {
        scene_id,
        node_id,
        property: property.to_string(),
        key: removed,
    })
}

fn set_frame_rate(
    txn: &mut ProjectDocument,
    num: u32,
    den: u32,
) -> Result<Command, CommandError> {
    let old = (txn.frame_rate.num, txn.frame_rate.den);
    let new_rate = anim_core::FrameRate::new(num, den)
        .map_err(|e| CommandError::Validation(format!("frame rate: {e}")))?;
    txn.frame_rate = new_rate;
    Ok(Command::SetFrameRate { num: old.0, den: old.1 })
}
