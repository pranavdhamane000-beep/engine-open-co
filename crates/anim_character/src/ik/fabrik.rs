//! FABRIK (Forward And Backward Reaching Inverse Kinematics) solver.

use crate::error::{CharError, CharResult};
use crate::normalize_angle;
use crate::pose::{LocalTransform, Pose};
use crate::skeleton::Skeleton;
use crate::solver_constants::*;
use crate::IkDiagnostic;
use crate::SolveOutcome;
use glam::DVec2;

pub fn solve_fabrik(
    skeleton: &Skeleton,
    pose: &mut Pose,
    chain: &crate::IkChain,
    target: &crate::IkTarget,
    diagnostics: &mut Vec<IkDiagnostic>,
    tolerance: f64,
) -> CharResult<SolveOutcome> {
    if chain.bones.len() < 2 {
        return Err(CharError::InvalidRigDefinition(
            "FABRIK requires at least 2 bones".into(),
        ));
    }

    let world = crate::fk_eval_world(skeleton, pose)?;
    let effector_id = chain.effector;
    let effector_world =
        world.get(effector_id).ok_or(CharError::BoneNotFound(effector_id))?;

    let target_pos = target.position;
    let initial_error = (effector_world.translation - target_pos).length();

    if initial_error <= tolerance {
        return Ok(SolveOutcome {
            success: true,
            iterations: 0,
            final_error: initial_error,
            diagnostics: diagnostics.clone(),
        });
    }

    let lengths: Vec<f64> = chain
        .bones
        .iter()
        .map(|id| skeleton.bone(*id).map(|b| b.length).unwrap_or(0.0))
        .collect();

    let mut current_pose_local = pose.clone();
    let mut converged = false;
    let mut final_error = initial_error;

    for _iter in 0..FABRIK_MAX_ITERATIONS {
        let world = crate::fk_eval_world(skeleton, &current_pose_local)?;

        let mut positions: Vec<DVec2> = chain
            .bones
            .iter()
            .map(|id| world.get(*id).map(|w| w.translation).unwrap_or(DVec2::ZERO))
            .collect();

        let last_idx = positions.len() - 1;
        positions[last_idx] = target_pos;

        // Backward pass: effector to root
        for i in (0..positions.len() - 1).rev() {
            let dir = positions[i + 1] - positions[i];
            let dist = dir.length();
            if dist > 1e-10 {
                positions[i] = positions[i + 1] - dir.normalize() * lengths[i];
            }
        }

        // Forward pass: root to effector
        for i in 1..positions.len() {
            let dir = positions[i] - positions[i - 1];
            let dist = dir.length();
            if dist > 1e-10 {
                positions[i] = positions[i - 1] + dir.normalize() * lengths[i - 1];
            }
        }

        let mut limit_hit = false;

        for (idx, &bone_id) in chain.bones.iter().enumerate() {
            let bone = skeleton.bone(bone_id)?;
            let local = current_pose_local
                .get(bone_id)
                .copied()
                .unwrap_or_else(LocalTransform::identity);

            if idx == 0 {
                if idx + 1 < positions.len() {
                    let to_next = positions[idx + 1] - positions[idx];
                    let new_rot = to_next.y.atan2(to_next.x);
                    let mut new_local = local;
                    new_local.rotation = normalize_angle(new_rot - bone.rest.rotation);
                    bone.limit.apply_to_local(&mut new_local);
                    if new_local.rotation != local.rotation {
                        limit_hit = true;
                    }
                    current_pose_local.set(bone_id, new_local);
                }
            } else {
                let parent_pos = positions[idx - 1];
                let this_pos = positions[idx];
                let to_this = this_pos - parent_pos;
                let new_rot = to_this.y.atan2(to_this.x);
                let mut new_local = local;
                new_local.rotation = normalize_angle(new_rot - bone.rest.rotation);
                bone.limit.apply_to_local(&mut new_local);
                if new_local.rotation != local.rotation {
                    limit_hit = true;
                }
                current_pose_local.set(bone_id, new_local);
            }
        }

        let new_world = crate::fk_eval_world(skeleton, &current_pose_local)?;
        let new_effector =
            new_world.get(effector_id).ok_or(CharError::BoneNotFound(effector_id))?;
        let error = (new_effector.translation - target_pos).length();
        final_error = error;

        if limit_hit {
            diagnostics.push(IkDiagnostic::LimitConflict {
                bone: chain.bones[0],
                requested: 0.0,
                clamped: 0.0,
            });
        }

        if error <= tolerance {
            converged = true;
            break;
        }
    }

    *pose = current_pose_local;

    if !converged {
        diagnostics.push(IkDiagnostic::Diverging {
            chain: chain.id,
            iterations: FABRIK_MAX_ITERATIONS,
        });
    }

    Ok(SolveOutcome {
        success: converged,
        iterations: FABRIK_MAX_ITERATIONS,
        final_error,
        diagnostics: diagnostics.clone(),
    })
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
    fn fabrik_reaches_target() {
        let rig = default_humanoid_rig(RigDefinitionId::from_u128(1));
        let skeleton = Skeleton::from_rig(&rig).expect("build skeleton");
        let mut pose = Pose::new();

        let chain = crate::IkChain {
            id: crate::IkChainId::from_u128(2),
            bones: vec![BoneId::from_u128(7), BoneId::from_u128(8), BoneId::from_u128(9)],
            effector: BoneId::from_u128(9),
            solver: crate::SolverKind::Fabrik { tolerance: 1e-4 },
            blend: 1.0,
        };

        let target = IkTarget {
            chain: chain.id,
            position: DVec2::new(-15.0, 10.0),
            orientation: 0.0,
            weight: 1.0,
        };

        let mut diagnostics = Vec::new();
        let result =
            solve_fabrik(&skeleton, &mut pose, &chain, &target, &mut diagnostics, 1e-4);
        assert!(result.is_ok());
    }
}
