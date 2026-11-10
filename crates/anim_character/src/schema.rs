//! Character rig schema (spec §27 Files and directories).
//!
//! Provides the authoritative authoring schema for character rigs.
//! WP12 consumes this schema; it does not invent rig asset authoring separately.

use crate::pose::LocalTransform;
use crate::skeleton::Bone;
use crate::socket::FaceSocketKind;
use crate::socket::Socket;
use crate::BoneId;
use crate::CharError;
use crate::RigDefinitionId;
use crate::SocketId;
use crate::SocketKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Rig definition schema version.
pub const RIG_SCHEMA_VERSION: u32 = 1;

/// Authored rig definition (spec §27 "rest-pose authoring schema").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RigDefinition {
    pub id: RigDefinitionId,
    pub schema_version: u32,
    pub name: String,
    /// Bones in topological order (parent before child).
    pub bones: Vec<Bone>,
    /// Sockets for attachment points (hand, face, weapon, etc.).
    pub sockets: Vec<Socket>,
}

impl RigDefinition {
    pub fn new(id: RigDefinitionId, name: String) -> Self {
        Self {
            id,
            schema_version: RIG_SCHEMA_VERSION,
            name,
            bones: Vec::new(),
            sockets: Vec::new(),
        }
    }

    pub fn validate(&self) -> crate::CharResult<()> {
        if self.schema_version != RIG_SCHEMA_VERSION {
            return Err(CharError::UnsupportedSchemaVersion(self.schema_version));
        }
        if self.bones.is_empty() {
            return Err(CharError::InvalidRigDefinition(
                "rig must have at least one bone".into(),
            ));
        }

        let mut seen = std::collections::BTreeSet::new();
        for bone in &self.bones {
            if !seen.insert(bone.id) {
                return Err(CharError::DuplicateBoneId(bone.id));
            }
        }

        let bone_ids: BTreeMap<BoneId, &Bone> =
            self.bones.iter().map(|b| (b.id, b)).collect();
        for bone in &self.bones {
            if let Some(parent) = bone.parent {
                if !bone_ids.contains_key(&parent) {
                    return Err(CharError::MissingParent { bone: bone.id, parent });
                }
            }
        }

        let root_count = self.bones.iter().filter(|b| b.parent.is_none()).count();
        if root_count != 1 {
            return Err(CharError::InvalidRigDefinition(format!(
                "rig must have exactly one root bone, found {root_count}"
            )));
        }

        for socket in &self.sockets {
            if !bone_ids.contains_key(&socket.bone) {
                return Err(CharError::InvalidRigDefinition(format!(
                    "socket {} references unknown bone {}",
                    socket.id, socket.bone
                )));
            }
        }

        Ok(())
    }

    pub fn build_skeleton(&self) -> crate::CharResult<crate::skeleton::Skeleton> {
        self.validate()?;
        crate::skeleton::Skeleton::from_rig(self)
    }
}

/// Default rest-pose authoring template for a basic humanoid (Test A reference).
pub fn default_humanoid_rig(id: RigDefinitionId) -> RigDefinition {
    let root = BoneId::from_u128(1); // hips/pelvis
    let spine1 = BoneId::from_u128(2); // spine lower
    let spine2 = BoneId::from_u128(3); // spine upper
    let neck = BoneId::from_u128(4); // neck
    let head = BoneId::from_u128(5); // head
    let l_clav = BoneId::from_u128(6); // left clavicle
    let l_uparm = BoneId::from_u128(7); // left upper arm
    let l_loarm = BoneId::from_u128(8); // left lower arm
    let l_hand = BoneId::from_u128(9); // left hand
    let r_clav = BoneId::from_u128(10); // right clavicle
    let r_uparm = BoneId::from_u128(11); // right upper arm
    let r_loarm = BoneId::from_u128(12); // right lower arm
    let r_hand = BoneId::from_u128(13); // right hand
    let l_thigh = BoneId::from_u128(14); // left thigh
    let l_shin = BoneId::from_u128(15); // left shin
    let l_foot = BoneId::from_u128(16); // left foot
    let r_thigh = BoneId::from_u128(17); // right thigh
    let r_shin = BoneId::from_u128(18); // right shin
    let r_foot = BoneId::from_u128(19); // right foot

    let bones = vec![
        Bone::new(
            root,
            None,
            LocalTransform::new(0.0, 0.0, 0.0, 1.0, 1.0, 0.0),
            0.0,
            crate::limits::JointLimit::Ball { cone_half_angle: 0.5 },
            true,
        ),
        Bone::new(
            spine1,
            Some(root),
            LocalTransform::new(0.0, 5.0, 0.0, 1.0, 1.0, 0.0),
            5.0,
            crate::limits::JointLimit::Revolute { min: -0.3, max: 0.3 },
            true,
        ),
        Bone::new(
            spine2,
            Some(spine1),
            LocalTransform::new(0.0, 5.0, 0.0, 1.0, 1.0, 0.0),
            5.0,
            crate::limits::JointLimit::Revolute { min: -0.3, max: 0.3 },
            true,
        ),
        Bone::new(
            neck,
            Some(spine2),
            LocalTransform::new(0.0, 5.0, 0.0, 1.0, 1.0, 0.0),
            3.0,
            crate::limits::JointLimit::Revolute { min: -0.8, max: 0.8 },
            true,
        ),
        Bone::new(
            head,
            Some(neck),
            LocalTransform::new(0.0, 3.0, 0.0, 1.0, 1.0, 0.0),
            2.0,
            crate::limits::JointLimit::Ball { cone_half_angle: 0.6 },
            true,
        ),
        Bone::new(
            l_clav,
            Some(spine2),
            LocalTransform::new(-2.0, 3.0, 0.0, 1.0, 1.0, 0.0),
            2.0,
            crate::limits::JointLimit::Revolute { min: -0.5, max: 0.5 },
            true,
        ),
        Bone::new(
            l_uparm,
            Some(l_clav),
            LocalTransform::new(-4.0, 0.0, 0.0, 1.0, 1.0, 0.0),
            8.0,
            crate::limits::JointLimit::Revolute { min: -2.0, max: 1.5 },
            true,
        ),
        Bone::new(
            l_loarm,
            Some(l_uparm),
            LocalTransform::new(-8.0, 0.0, 0.0, 1.0, 1.0, 0.0),
            7.0,
            crate::limits::JointLimit::Revolute { min: -0.1, max: 2.5 },
            true,
        ),
        Bone::new(
            l_hand,
            Some(l_loarm),
            LocalTransform::new(-7.0, 0.0, 0.0, 1.0, 1.0, 0.0),
            2.0,
            crate::limits::JointLimit::Ball { cone_half_angle: 0.8 },
            true,
        ),
        Bone::new(
            r_clav,
            Some(spine2),
            LocalTransform::new(2.0, 3.0, 0.0, 1.0, 1.0, 0.0),
            2.0,
            crate::limits::JointLimit::Revolute { min: -0.5, max: 0.5 },
            true,
        ),
        Bone::new(
            r_uparm,
            Some(r_clav),
            LocalTransform::new(4.0, 0.0, 0.0, 1.0, 1.0, 0.0),
            8.0,
            crate::limits::JointLimit::Revolute { min: -1.5, max: 2.0 },
            true,
        ),
        Bone::new(
            r_loarm,
            Some(r_uparm),
            LocalTransform::new(8.0, 0.0, 0.0, 1.0, 1.0, 0.0),
            7.0,
            crate::limits::JointLimit::Revolute { min: -2.5, max: 0.1 },
            true,
        ),
        Bone::new(
            r_hand,
            Some(r_loarm),
            LocalTransform::new(7.0, 0.0, 0.0, 1.0, 1.0, 0.0),
            2.0,
            crate::limits::JointLimit::Ball { cone_half_angle: 0.8 },
            true,
        ),
        Bone::new(
            l_thigh,
            Some(root),
            LocalTransform::new(-1.5, -5.0, 0.0, 1.0, 1.0, 0.0),
            10.0,
            crate::limits::JointLimit::Revolute { min: -0.2, max: 1.8 },
            true,
        ),
        Bone::new(
            l_shin,
            Some(l_thigh),
            LocalTransform::new(0.0, -10.0, 0.0, 1.0, 1.0, 0.0),
            9.0,
            crate::limits::JointLimit::Revolute { min: -0.1, max: 0.0 },
            true,
        ),
        Bone::new(
            l_foot,
            Some(l_shin),
            LocalTransform::new(0.0, -9.0, 0.0, 1.0, 1.0, 0.0),
            3.0,
            crate::limits::JointLimit::Revolute { min: -0.5, max: 0.5 },
            true,
        ),
        Bone::new(
            r_thigh,
            Some(root),
            LocalTransform::new(1.5, -5.0, 0.0, 1.0, 1.0, 0.0),
            10.0,
            crate::limits::JointLimit::Revolute { min: -0.2, max: 1.8 },
            true,
        ),
        Bone::new(
            r_shin,
            Some(r_thigh),
            LocalTransform::new(0.0, -10.0, 0.0, 1.0, 1.0, 0.0),
            9.0,
            crate::limits::JointLimit::Revolute { min: -0.1, max: 0.0 },
            true,
        ),
        Bone::new(
            r_foot,
            Some(r_shin),
            LocalTransform::new(0.0, -9.0, 0.0, 1.0, 1.0, 0.0),
            3.0,
            crate::limits::JointLimit::Revolute { min: -0.5, max: 0.5 },
            true,
        ),
    ];

    let sockets = vec![
        Socket::new(
            SocketId::from_u128(1),
            l_hand,
            LocalTransform::identity(),
            SocketKind::Hand { is_left: true },
        ),
        Socket::new(
            SocketId::from_u128(2),
            r_hand,
            LocalTransform::identity(),
            SocketKind::Hand { is_left: false },
        ),
        Socket::new(
            SocketId::from_u128(3),
            head,
            LocalTransform::new(0.0, 1.5, 0.0, 1.0, 1.0, 0.0),
            SocketKind::Face { kind: FaceSocketKind::Head },
        ),
        Socket::new(
            SocketId::from_u128(4),
            l_foot,
            LocalTransform::identity(),
            SocketKind::Foot { is_left: true },
        ),
        Socket::new(
            SocketId::from_u128(5),
            r_foot,
            LocalTransform::identity(),
            SocketKind::Foot { is_left: false },
        ),
    ];

    RigDefinition {
        id,
        schema_version: RIG_SCHEMA_VERSION,
        name: "Test A Humanoid".into(),
        bones,
        sockets,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::default_humanoid_rig;
    use crate::skeleton::Skeleton;
    use crate::RigDefinitionId;

    #[test]
    fn default_humanoid_rig_validates() {
        let rig = default_humanoid_rig(RigDefinitionId::from_u128(1));
        assert!(rig.validate().is_ok());
        assert_eq!(rig.bones.len(), 19);
        assert_eq!(rig.sockets.len(), 5);
    }

    #[test]
    fn humanoid_rig_builds_skeleton() {
        let rig = default_humanoid_rig(RigDefinitionId::from_u128(1));
        let skeleton = rig.build_skeleton().expect("build skeleton");
        assert_eq!(skeleton.len(), 19);
        assert_eq!(skeleton.topological_order().len(), 19);
        assert_eq!(skeleton.topological_order()[0], BoneId::from_u128(1));
    }
}
