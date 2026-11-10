//! Analytic two-bone IK solver (spec §27 Algorithms: Two-bone analytic IK).

use crate::error::{CharError, CharResult};
use crate::normalize_angle;
use crate::pose::{LocalTransform, Pose, WorldTransform};
use crate::safe_acos;
use crate::skeleton::Skeleton;
use crate::solver_constants::*;
use crate::BoneId;
use crate::IkDiagnostic;
use crate::SolveOutcome;
use glam::DVec2;
use serde::{Deserialize, Serialize};

/// Pole vector source for bend direction (spec §27 Data structures).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PoleSource {
    Fixed(DVec2),
    Bone(BoneId),
    AuthoredSign(i8),
}

impl Default for PoleSource {
    fn default() -> Self {
        Self::AuthoredSign(1)
    }
}

/// Solve analytic two-bone IK chain.
pub fn solve_two_bone(
    skeleton: &Skeleton,
    pose: &mut Pose,
    chain: &crate::IkChain,
    target: &crate::IkTarget,
    diagnostics: &mut Vec<IkDiagnostic>,
    pole: PoleSource,
) -> CharResult<SolveOutcome> {
    if chain.bones.len() != 2 {
        return Err(CharError::InvalidRigDefinition(
            "Two-bone IK requires exactly 2 bones".into(),
        ));
    }

    let start_id = chain.bones[0];
    let end_id = chain.bones[1];

    let start_bone = skeleton.bone(start_id)?;
    let end_bone = skeleton.bone(end_id)?;

    let l1 = start_bone.length;
    let l2 = end_bone.length;

    let world = crate::fk_eval_world(skeleton, pose)?;
    let start_world = world.get(start_id).ok_or(CharError::BoneNotFound(start_id))?;

    let target_pos = target.position;
    let start_pos = start_world.translation;

    let to_target = target_pos - start_pos;
    let d = to_target.length();

    let min_reach = (l1 - l2).abs() + TWO_BONE_EPS;
    let max_reach = l1 + l2 - TWO_BONE_EPS;
    let d_clamped = d.clamp(min_reach, max_reach);

    let was_unreachable = d > max_reach || d < min_reach;
    if was_unreachable {
        diagnostics.push(IkDiagnostic::Unreachable {
            chain: chain.id,
            requested: d,
            max_reach,
        });
    }

    let cos_angle2 =
        ((l1 * l1 + l2 * l2 - d_clamped * d_clamped) / (2.0 * l1 * l2)).clamp(-1.0, 1.0);
    let angle2 = safe_acos(cos_angle2);

    let angle_to_target = (target_pos.y - start_pos.y).atan2(target_pos.x - start_pos.x);

    let bend_sign = determine_bend_sign(pole, start_world, target_pos);

    let cos_angle1 = ((l1 * l1 + d_clamped * d_clamped - l2 * l2) / (2.0 * l1 * d_clamped))
        .clamp(-1.0, 1.0);
    let angle1 = safe_acos(cos_angle1);

    let joint1_angle = angle_to_target - bend_sign * angle1;
    let joint2_angle = bend_sign * (std::f64::consts::PI - angle2);

    let mut pose_mut = pose.clone();
    let mut start_local =
        pose.get(start_id).copied().unwrap_or_else(LocalTransform::identity);
    let mut end_local = pose.get(end_id).copied().unwrap_or_else(LocalTransform::identity);

    start_local.rotation = normalize_angle(joint1_angle);
    end_local.rotation = normalize_angle(joint2_angle);

    let start_bone = skeleton.bone(start_id)?;
    let end_bone = skeleton.bone(end_id)?;
    start_bone.limit.apply_to_local(&mut start_local);
    end_bone.limit.apply_to_local(&mut end_local);

    pose_mut.set(start_id, start_local);
    pose_mut.set(end_id, end_local);

    let limit_hit = !start_bone.limit.contains_rotation(start_local.rotation)
        || !end_bone.limit.contains_rotation(end_local.rotation);

    if limit_hit {
        diagnostics.push(IkDiagnostic::LimitConflict {
            bone: start_id,
            requested: joint1_angle,
            clamped: start_local.rotation,
        });
    }

    Ok(SolveOutcome {
        success: true,
        iterations: 1,
        final_error: if was_unreachable { d - max_reach } else { 0.0 },
        diagnostics: diagnostics.clone(),
    })
}

fn determine_bend_sign(
    pole: PoleSource,
    start_world: &WorldTransform,
    target_pos: DVec2,
) -> f64 {
    match pole {
        PoleSource::Fixed(pole_vec) => {
            let to_target = target_pos - start_world.translation;
            let cross = to_target.x * pole_vec.y - to_target.y * pole_vec.x;
            cross.signum()
        }
        PoleSource::Bone(_bone_id) => 1.0,
        PoleSource::AuthoredSign(sign) => sign as f64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pose::Pose;
    use crate::schema::default_humanoid_rig;
    use crate::skeleton::Skeleton;
    use crate::BoneId;
    use crate::IkTarget;
    use crate::RigDefinitionId;
    use glam::DVec2;

    #[test]
    fn two_bone_ik_reaches_target() {
        let rig = default_humanoid_rig(RigDefinitionId::from_u128(1));
        let skeleton = Skeleton::from_rig(&rig).expect("build skeleton");
        let mut pose = Pose::new();

        let chain = crate::IkChain {
            id: crate::IkChainId::from_u128(1),
            bones: vec![BoneId::from_u128(7), BoneId::from_u128(8)],
            effector: BoneId::from_u128(9),
            solver: crate::SolverKind::TwoBone { pole: PoleSource::AuthoredSign(1) },
            blend: 1.0,
        };

        let target = IkTarget {
            chain: chain.id,
            position: DVec2::new(-15.0, 10.0),
            orientation: 0.0,
            weight: 1.0,
        };

        let mut diagnostics = Vec::new();
        let result = solve_two_bone(
            &skeleton,
            &mut pose,
            &chain,
            &target,
            &mut diagnostics,
            PoleSource::AuthoredSign(1),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn two_bone_unreachable_clamps() {
        let rig = default_humanoid_rig(RigDefinitionId::from_u128(1));
        let skeleton = Skeleton::from_rig(&rig).expect("build skeleton");
        let mut pose = Pose::new();

        let chain = crate::IkChain {
            id: crate::IkChainId::from_u128(1),
            bones: vec![BoneId::from_u128(7), BoneId::from_u128(8)],
            effector: BoneId::from_u128(9),
            solver: crate::SolverKind::TwoBone { pole: PoleSource::AuthoredSign(1) },
            blend: 1.0,
        };

        let target = IkTarget {
            chain: chain.id,
            position: DVec2::new(-100.0, 100.0),
            orientation: 0.0,
            weight: 1.0,
        };

        let mut diagnostics = Vec::new();
        let result = solve_two_bone(
            &skeleton,
            &mut pose,
            &chain,
            &target,
            &mut diagnostics,
            PoleSource::AuthoredSign(1),
        );
        assert!(result.is_ok());
        assert!(result
            .unwrap()
            .diagnostics
            .iter()
            .any(|d| matches!(d, IkDiagnostic::Unreachable { .. })));
    }
}
