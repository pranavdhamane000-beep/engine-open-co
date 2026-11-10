//! Pose types and local/world transform evaluation (spec §27 Data structures).
//!
//! Uses right-handed planar coordinates: X right, Y up, CCW-positive radians.
//! Bone pivot is at its head (start of bone).

use crate::error::CharError;
use crate::BoneId;
use crate::Skeleton;
use glam::DMat3;
use glam::DVec2;
use glam::DVec3;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Local bone transform relative to its parent (spec §27 Data structures).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LocalTransform {
    pub translation: DVec2,
    pub rotation: f64,
    pub scale: DVec2,
    pub depth: f64,
}

impl LocalTransform {
    pub fn identity() -> Self {
        Self { translation: DVec2::ZERO, rotation: 0.0, scale: DVec2::ONE, depth: 0.0 }
    }

    pub fn new(x: f64, y: f64, rot: f64, sx: f64, sy: f64, depth: f64) -> Self {
        Self {
            translation: DVec2::new(x, y),
            rotation: rot,
            scale: DVec2::new(sx, sy),
            depth,
        }
    }

    pub fn validate(&self) -> crate::CharResult<()> {
        if !self.translation.x.is_finite() || !self.translation.y.is_finite() {
            return Err(CharError::NonFinite {
                context: "translation".into(),
                value: f64::NAN,
            });
        }
        if !self.rotation.is_finite() {
            return Err(CharError::NonFinite {
                context: "rotation".into(),
                value: f64::NAN,
            });
        }
        if !self.scale.x.is_finite() || !self.scale.y.is_finite() {
            return Err(CharError::NonFinite { context: "scale".into(), value: f64::NAN });
        }
        if !self.depth.is_finite() {
            return Err(CharError::NonFinite { context: "depth".into(), value: f64::NAN });
        }
        Ok(())
    }

    pub fn to_matrix(&self) -> DMat3 {
        let c = self.rotation.cos();
        let s = self.rotation.sin();
        let sx = self.scale.x;
        let sy = self.scale.y;
        let tx = self.translation.x;
        let ty = self.translation.y;

        DMat3::from_cols(
            DVec3::new(c * sx, s * sy, 0.0),
            DVec3::new(-s * sx, c * sy, 0.0),
            DVec3::new(tx, ty, 1.0),
        )
    }
}

/// World-space transform (absolute, not relative to parent).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WorldTransform {
    pub translation: DVec2,
    pub rotation: f64,
    pub scale: DVec2,
    pub depth: f64,
}

impl WorldTransform {
    pub fn identity() -> Self {
        Self { translation: DVec2::ZERO, rotation: 0.0, scale: DVec2::ONE, depth: 0.0 }
    }
}

/// Immutable pose: local transforms for all bones (spec §27 Pose).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pose {
    pub locals: BTreeMap<BoneId, LocalTransform>,
}

impl Pose {
    pub fn new() -> Self {
        Self { locals: BTreeMap::new() }
    }

    pub fn with_locals(locals: BTreeMap<BoneId, LocalTransform>) -> Self {
        Self { locals }
    }

    pub fn get(&self, id: BoneId) -> Option<&LocalTransform> {
        self.locals.get(&id)
    }

    pub fn set(&mut self, id: BoneId, transform: LocalTransform) {
        self.locals.insert(id, transform);
    }

    pub fn remove(&mut self, id: BoneId) -> Option<LocalTransform> {
        self.locals.remove(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&BoneId, &LocalTransform)> {
        self.locals.iter()
    }

    pub fn len(&self) -> usize {
        self.locals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.locals.is_empty()
    }
}

impl Default for Pose {
    fn default() -> Self {
        Self::new()
    }
}

/// Immutable world pose: world transforms for all bones (spec §27 WorldPose).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldPose {
    pub world: BTreeMap<BoneId, WorldTransform>,
}

impl WorldPose {
    pub fn new() -> Self {
        Self { world: BTreeMap::new() }
    }

    pub fn with_world(world: BTreeMap<BoneId, WorldTransform>) -> Self {
        Self { world }
    }

    pub fn get(&self, id: BoneId) -> Option<&WorldTransform> {
        self.world.get(&id)
    }

    pub fn set(&mut self, id: BoneId, transform: WorldTransform) {
        self.world.insert(id, transform);
    }

    pub fn len(&self) -> usize {
        self.world.len()
    }

    pub fn is_empty(&self) -> bool {
        self.world.is_empty()
    }
}

impl Default for WorldPose {
    fn default() -> Self {
        Self::new()
    }
}

/// Forward kinematics evaluation: parent-before-child topological order (spec §27 Algorithms).
/// world[bone] = world[parent] * rest_adjustment * local[bone]
pub fn fk_eval_world(skeleton: &Skeleton, pose: &Pose) -> crate::CharResult<WorldPose> {
    let mut world = BTreeMap::new();

    for &bone_id in skeleton.topological_order() {
        let bone = skeleton.bone(bone_id)?;
        let local = pose.get(bone_id).copied().unwrap_or_else(LocalTransform::identity);
        local.validate()?;

        let rest = bone.rest;
        rest.validate()?;

        let combined = combine_local_with_rest(local, rest);

        if let Some(parent_id) = bone.parent {
            let parent_world =
                world.get(&parent_id).ok_or(CharError::BoneNotFound(parent_id))?;
            world.insert(bone_id, multiply_world(parent_world, combined));
        } else {
            world.insert(bone_id, combined);
        }
    }

    Ok(WorldPose::with_world(world))
}

fn combine_local_with_rest(local: LocalTransform, rest: LocalTransform) -> WorldTransform {
    let m_local = local.to_matrix();
    let m_rest = rest.to_matrix();
    let m = m_rest * m_local;
    matrix_to_world_transform(m)
}

fn multiply_world(parent: &WorldTransform, child: WorldTransform) -> WorldTransform {
    let m_parent = world_to_matrix(parent);
    let m_child = world_to_matrix(&child);
    let m = m_parent * m_child;
    matrix_to_world_transform(m)
}

pub fn world_to_matrix(w: &WorldTransform) -> DMat3 {
    let c = w.rotation.cos();
    let s = w.rotation.sin();
    let sx = w.scale.x;
    let sy = w.scale.y;
    let tx = w.translation.x;
    let ty = w.translation.y;

    DMat3::from_cols(
        DVec3::new(c * sx, s * sy, 0.0),
        DVec3::new(-s * sx, c * sy, 0.0),
        DVec3::new(tx, ty, 1.0),
    )
}

fn matrix_to_world_transform(m: DMat3) -> WorldTransform {
    let scale_x = m.x_axis.length();
    let scale_y = m.y_axis.length();
    let rotation = m.y_axis.y.atan2(m.x_axis.y);
    let translation = DVec2::new(m.z_axis.x, m.z_axis.y);

    WorldTransform {
        translation,
        rotation,
        scale: DVec2::new(scale_x, scale_y),
        depth: 0.0,
    }
}

impl From<LocalTransform> for WorldTransform {
    fn from(l: LocalTransform) -> Self {
        Self {
            translation: l.translation,
            rotation: l.rotation,
            scale: l.scale,
            depth: l.depth,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::default_humanoid_rig;
    use crate::skeleton::Skeleton;
    use crate::RigDefinitionId;

    #[test]
    fn fk_eval_world_works() {
        let rig = default_humanoid_rig(RigDefinitionId::from_u128(1));
        let skeleton = Skeleton::from_rig(&rig).expect("build skeleton");
        let pose = Pose::new();
        let world = fk_eval_world(&skeleton, &pose).expect("fk eval");

        assert_eq!(world.len(), skeleton.len());

        let root = BoneId::from_u128(1);
        let root_world = world.get(root).expect("root in world");
        assert!((root_world.translation.x - 0.0).abs() < 1e-6);
        assert!((root_world.translation.y - 0.0).abs() < 1e-6);
    }

    #[test]
    fn local_transform_identity() {
        let id = LocalTransform::identity();
        assert_eq!(id.translation, DVec2::ZERO);
        assert_eq!(id.rotation, 0.0);
        assert_eq!(id.scale, DVec2::ONE);
        assert_eq!(id.depth, 0.0);
    }

    #[test]
    fn local_transform_validate_catches_nan() {
        let mut bad = LocalTransform::identity();
        bad.rotation = f64::NAN;
        assert!(bad.validate().is_err());
    }
}
