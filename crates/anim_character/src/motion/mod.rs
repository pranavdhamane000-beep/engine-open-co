//! Root motion, contact locks and stabilization (spec §27 Data structures, Algorithms).

use crate::error::{CharError, CharResult};
use crate::skeleton::Skeleton;
use crate::BoneId;
use anim_core::Rational;
use glam::DVec2;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Root transform.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RootTransform {
    pub position: DVec2,
    pub rotation: f64,
    pub depth: f64,
}

impl RootTransform {
    pub fn identity() -> Self {
        Self { position: DVec2::ZERO, rotation: 0.0, depth: 0.0 }
    }
}

/// Root motion delta between two poses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RootDelta {
    pub translation: DVec2,
    pub rotation: f64,
    pub duration: Rational,
}

impl RootDelta {
    pub fn zero(duration: Rational) -> Self {
        Self { translation: DVec2::ZERO, rotation: 0.0, duration }
    }
}

/// Contact lock release condition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LockRelease {
    /// Release at specific frame.
    Frame(anim_core::FramePos),
    /// Release when condition met (custom).
    Condition(String),
    /// Never release (manual only).
    Manual,
}

/// Contact lock for hand/foot pinning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContactLock {
    pub active: bool,
    pub world_pin: DVec2,
    pub release: LockRelease,
    pub bone: BoneId,
}

impl ContactLock {
    pub fn new(bone: BoneId, world_pin: DVec2) -> Self {
        Self { active: true, world_pin, release: LockRelease::Manual, bone }
    }
}

/// Extract root motion between two poses.
pub fn extract_root_motion(
    skeleton: &Skeleton,
    from: &crate::pose::Pose,
    to: &crate::pose::Pose,
    duration: Rational,
) -> CharResult<RootDelta> {
    let from_world = crate::fk_eval_world(skeleton, from)?;
    let to_world = crate::fk_eval_world(skeleton, to)?;

    let root_id = skeleton
        .topological_order()
        .first()
        .copied()
        .ok_or(CharError::InvalidRigDefinition("skeleton has no root".into()))?;

    let from_root = from_world.get(root_id).ok_or(CharError::BoneNotFound(root_id))?;
    let to_root = to_world.get(root_id).ok_or(CharError::BoneNotFound(root_id))?;

    let translation = to_root.translation - from_root.translation;
    let rotation = crate::normalize_angle(to_root.rotation - from_root.rotation);

    Ok(RootDelta { translation, rotation, duration })
}

/// Apply root motion to a pose.
pub fn apply_root_motion(
    pose: &mut crate::pose::Pose,
    delta: &RootDelta,
    skeleton: &Skeleton,
) -> CharResult<()> {
    let root_id = skeleton
        .topological_order()
        .first()
        .copied()
        .ok_or(CharError::InvalidRigDefinition("skeleton has no root".into()))?;

    if let Some(local) = pose.get(root_id) {
        let mut new_local = *local;
        new_local.translation.x += delta.translation.x;
        new_local.translation.y += delta.translation.y;
        new_local.rotation = crate::normalize_angle(new_local.rotation + delta.rotation);
        pose.set(root_id, new_local);
    }
    Ok(())
}

/// Full-body stabilization: ensure contact pins remain fixed while root moves.
pub fn stabilize_contacts(
    skeleton: &Skeleton,
    pose: &mut crate::pose::Pose,
    contacts: &BTreeMap<BoneId, ContactLock>,
    _root_delta: &RootDelta,
) -> CharResult<()> {
    if contacts.is_empty() {
        return Ok(());
    }

    let mut stabilized = pose.clone();

    for _iter in 0..crate::solver_constants::STABILIZATION_MAX_ITERATIONS {
        let world = crate::fk_eval_world(skeleton, &stabilized)?;
        let mut max_correction: f64 = 0.0;

        for (bone_id, contact) in contacts {
            if !contact.active {
                continue;
            }
            let bone_world =
                world.get(*bone_id).ok_or(CharError::BoneNotFound(*bone_id))?;
            let error = (bone_world.translation - contact.world_pin).length();
            max_correction = max_correction.max(error);

            if error > crate::solver_constants::STABILIZATION_TOLERANCE {
                let to_pin = contact.world_pin - bone_world.translation;
                apply_stabilization_correction(
                    skeleton,
                    &mut stabilized,
                    *bone_id,
                    to_pin,
                )?;
            }
        }

        if max_correction < crate::solver_constants::STABILIZATION_TOLERANCE {
            break;
        }
    }

    *pose = stabilized;
    Ok(())
}

fn apply_stabilization_correction(
    skeleton: &Skeleton,
    pose: &mut crate::pose::Pose,
    bone_id: BoneId,
    correction: DVec2,
) -> CharResult<()> {
    let mut current = bone_id;
    let remaining = correction;

    while let Ok(bone) = skeleton.bone(current) {
        if let Some(local) = pose.get(current) {
            let mut new_local = *local;
            let factor = 0.5;
            new_local.translation.x += remaining.x * factor;
            new_local.translation.y += remaining.y * factor;
            pose.set(current, new_local);
        }

        if let Some(parent) = bone.parent {
            current = parent;
        } else {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pose::Pose;
    use crate::schema::default_humanoid_rig;
    use crate::skeleton::Skeleton;
    use crate::RigDefinitionId;
    use anim_core::Rational;
    use glam::DVec2;

    #[test]
    fn root_transform_identity() {
        let rt = RootTransform::identity();
        assert_eq!(rt.position, DVec2::ZERO);
        assert_eq!(rt.rotation, 0.0);
    }

    #[test]
    fn root_delta_zero() {
        let dur = Rational::new(1, 24).unwrap();
        let rd = RootDelta::zero(dur);
        assert_eq!(rd.translation, DVec2::ZERO);
        assert_eq!(rd.rotation, 0.0);
    }

    #[test]
    fn contact_lock_new() {
        let bone = BoneId::from_u128(1);
        let pin = DVec2::new(10.0, 0.0);
        let lock = ContactLock::new(bone, pin);
        assert!(lock.active);
        assert_eq!(lock.world_pin, pin);
        assert_eq!(lock.bone, bone);
    }
}
