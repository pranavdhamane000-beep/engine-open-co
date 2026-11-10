//! Socket contracts for hand/face/weapon attachment (spec §27 Data structures, Interfaces).

use crate::error::{CharError, CharResult};
use crate::pose::{WorldPose, WorldTransform};
use crate::skeleton::Skeleton;
use crate::BoneId;
use crate::SocketId;
use glam::DVec2;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Socket kind for different attachment types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SocketKind {
    Hand { is_left: bool },
    Foot { is_left: bool },
    Face { kind: FaceSocketKind },
    Weapon { slot: u8 },
    Custom(u16),
}

impl Default for SocketKind {
    fn default() -> Self {
        Self::Custom(0)
    }
}

/// Face socket subtypes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FaceSocketKind {
    Head,
    Jaw,
    LeftEye,
    RightEye,
    LeftEyebrow,
    RightEyebrow,
    Mouth,
    Chin,
    Nose,
}

/// Socket definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Socket {
    pub id: SocketId,
    pub bone: BoneId,
    pub local: crate::pose::LocalTransform,
    pub kind: SocketKind,
}

impl Socket {
    pub fn new(
        id: SocketId,
        bone: BoneId,
        local: crate::pose::LocalTransform,
        kind: SocketKind,
    ) -> Self {
        Self { id, bone, local, kind }
    }

    pub fn validate(&self, skeleton: &Skeleton) -> CharResult<()> {
        skeleton.bone(self.bone)?;
        self.local.validate()?;
        Ok(())
    }
}

pub fn resolve_socket(
    world: &WorldPose,
    _skeleton: &Skeleton,
    socket: &Socket,
) -> CharResult<WorldTransform> {
    let bone_world = world.get(socket.bone).ok_or(CharError::BoneNotFound(socket.bone))?;
    let socket_local = socket.local;

    let m_bone = crate::pose::world_to_matrix(bone_world);
    let m_local = socket_local.to_matrix();
    let m = m_bone * m_local;

    let translation = DVec2::new(m.z_axis.x, m.z_axis.y);
    let rotation = m.y_axis.y.atan2(m.x_axis.y);
    let scale_x = m.x_axis.length();
    let scale_y = m.y_axis.length();

    Ok(WorldTransform {
        translation,
        rotation,
        scale: DVec2::new(scale_x, scale_y),
        depth: bone_world.depth + socket_local.depth,
    })
}

pub fn resolve_all_sockets(
    world: &WorldPose,
    skeleton: &Skeleton,
    sockets: &std::collections::BTreeMap<SocketId, Socket>,
) -> CharResult<std::collections::BTreeMap<SocketId, WorldTransform>> {
    let mut result = BTreeMap::new();
    for (id, socket) in sockets {
        let transform = resolve_socket(world, skeleton, socket)?;
        result.insert(*id, transform);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pose::Pose;
    use crate::schema::default_humanoid_rig;
    use crate::skeleton::Skeleton;
    use crate::RigDefinitionId;
    use glam::DVec2;

    #[test]
    fn socket_resolve_works() {
        let rig = default_humanoid_rig(RigDefinitionId::from_u128(1));
        let skeleton = Skeleton::from_rig(&rig).expect("build skeleton");
        let pose = Pose::new();
        let world = crate::fk_eval_world(&skeleton, &pose).expect("fk eval");

        let hand_socket = rig
            .sockets
            .iter()
            .find(|s| matches!(s.kind, SocketKind::Hand { .. }))
            .expect("hand socket");
        let hand_world = resolve_socket(&world, &skeleton, hand_socket).expect("resolve");

        assert!(hand_world.translation.x.is_finite());
        assert!(hand_world.translation.y.is_finite());
    }

    #[test]
    fn socket_validation_catches_bad_bone() {
        let mut socket = Socket::new(
            SocketId::from_u128(99),
            crate::BoneId::from_u128(999),
            crate::pose::LocalTransform::identity(),
            SocketKind::Hand { is_left: true },
        );
        let rig = default_humanoid_rig(RigDefinitionId::from_u128(1));
        let skeleton = Skeleton::from_rig(&rig).expect("build");
        assert!(socket.validate(&skeleton).is_err());
    }
}
