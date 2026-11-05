//! Versioned JSON command payload models (spec 12.2: "All command payload
//! types have versioned JSON schemas"; schemas/commands/ holds the
//! documentation contracts).
//!
//! The models are 1:1 with `Command` variants minus the type tag — the
//! registry has already consumed the tag. Serde is the ENFORCING parser;
//! these mirrors exist so the dispatch table can name each payload type.

use crate::command::Command;
use anim_assets::AssetManifestEntry;
use anim_core::{AssetId, NodeId, SceneId};
use anim_project::PropertyKey;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpsertAssetPayload {
    pub entry: AssetManifestEntry,
}

impl From<UpsertAssetPayload> for Command {
    fn from(p: UpsertAssetPayload) -> Self {
        Command::UpsertAsset { entry: p.entry }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplaceAssetPayload {
    pub entry: AssetManifestEntry,
}

impl From<ReplaceAssetPayload> for Command {
    fn from(p: ReplaceAssetPayload) -> Self {
        Command::ReplaceAsset { entry: p.entry }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoveAssetPayload {
    pub asset_id: AssetId,
}

impl From<RemoveAssetPayload> for Command {
    fn from(p: RemoveAssetPayload) -> Self {
        Command::RemoveAsset { asset_id: p.asset_id }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpsertScenePayload {
    pub scene_id: SceneId,
    pub name: String,
}

impl From<UpsertScenePayload> for Command {
    fn from(p: UpsertScenePayload) -> Self {
        Command::UpsertScene { scene_id: p.scene_id, name: p.name }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenameScenePayload {
    pub scene_id: SceneId,
    pub name: String,
}

impl From<RenameScenePayload> for Command {
    fn from(p: RenameScenePayload) -> Self {
        Command::RenameScene { scene_id: p.scene_id, name: p.name }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoveScenePayload {
    pub scene_id: SceneId,
}

impl From<RemoveScenePayload> for Command {
    fn from(p: RemoveScenePayload) -> Self {
        Command::RemoveScene { scene_id: p.scene_id }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpsertNodePayload {
    pub scene_id: SceneId,
    pub node_id: NodeId,
    pub name: String,
    pub parent: Option<NodeId>,
    pub asset: Option<AssetId>,
}

impl From<UpsertNodePayload> for Command {
    fn from(p: UpsertNodePayload) -> Self {
        Command::UpsertNode {
            scene_id: p.scene_id,
            node_id: p.node_id,
            name: p.name,
            parent: p.parent,
            asset: p.asset,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoveNodePayload {
    pub scene_id: SceneId,
    pub node_id: NodeId,
}

impl From<RemoveNodePayload> for Command {
    fn from(p: RemoveNodePayload) -> Self {
        Command::RemoveNode { scene_id: p.scene_id, node_id: p.node_id }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetNodePropertyPayload {
    pub scene_id: SceneId,
    pub node_id: NodeId,
    pub property: String,
    pub key: PropertyKey,
}

impl From<SetNodePropertyPayload> for Command {
    fn from(p: SetNodePropertyPayload) -> Self {
        Command::SetNodeProperty {
            scene_id: p.scene_id,
            node_id: p.node_id,
            property: p.property,
            key: p.key,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoveNodePropertyKeyPayload {
    pub scene_id: SceneId,
    pub node_id: NodeId,
    pub property: String,
    pub key_time: anim_core::Rational,
}

impl From<RemoveNodePropertyKeyPayload> for Command {
    fn from(p: RemoveNodePropertyKeyPayload) -> Self {
        Command::RemoveNodePropertyKey {
            scene_id: p.scene_id,
            node_id: p.node_id,
            property: p.property,
            key_time: p.key_time,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetFrameRatePayload {
    pub num: u32,
    pub den: u32,
}

impl From<SetFrameRatePayload> for Command {
    fn from(p: SetFrameRatePayload) -> Self {
        Command::SetFrameRate { num: p.num, den: p.den }
    }
}
