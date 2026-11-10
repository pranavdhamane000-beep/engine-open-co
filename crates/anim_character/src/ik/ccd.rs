//! CCD (Cyclic Coordinate Descent) solver.

use crate::error::{CharError, CharResult};
use crate::pose::Pose;
use crate::skeleton::Skeleton;
use crate::solver_constants::*;
use crate::IkDiagnostic;
use crate::SolveOutcome;

pub fn solve_ccd(
    skeleton: &Skeleton,
    pose: &mut Pose,
    chain: &crate::IkChain,
    target: &crate::IkTarget,
    diagnostics: &mut Vec<IkDiagnostic>,
    gain: f64,
) -> CharResult<SolveOutcome> {
    if chain.bones.len() < 2 {
        return Err(CharError::InvalidRigDefinition(
            "CCD requires at least 2 bones".into(),
        ));
    }

    let effector_id = chain.effector;
    let target_pos = target.position;

    let world = crate::fk_eval_world(skeleton, pose)?;
    let effector_world =
        world.get(effector_id).ok_or(CharError::BoneNotFound(effector_id))?;

    let initial_error = (effector_world.translation - target_pos).length();

    if initial_error <= 1e-6 {
        return Ok(SolveOutcome {
            success: true,
            iterations: 0,
            final_error: initial_error,
            diagnostics: diagnostics.clone(),
        });
    }

    let mut current_pose = pose.clone();
    let mut final_error = initial_error;
    let mut converged = false;

    for _iter in 0..CCD_MAX_ITERATIONS {
        for idx in (1..chain.bones.len()).rev() {
            let world = crate::fk_eval_world(skeleton, &current_pose)?;

            let bone_id = chain.bones[idx];

            let bone = skeleton.bone(bone_id)?;
            let bone_world = world.get(bone_id).ok_or(CharError::BoneNotFound(bone_id))?;

            let effector_world =
                world.get(effector_id).ok_or(CharError::BoneNotFound(effector_id))?;
            let to_effector = effector_world.translation - bone_world.translation;
            let to_target = target_pos - bone_world.translation;

            let eff_len = to_effector.length();
            let tgt_len = to_target.length();

            if eff_len < 1e-6 || tgt_len < 1e-6 {
                continue;
            }

            let cos_angle = (to_effector.dot(to_target)) / (eff_len * tgt_len);
            let angle = crate::safe_acos(cos_angle.clamp(-1.0, 1.0));

            let cross = to_effector.x * to_target.y - to_effector.y * to_target.x;
            let sign = if cross >= 0.0 { 1.0 } else { -1.0 };

            let rotation_delta = sign * angle * CCD_GAIN * gain;

            let mut local = current_pose
                .get(bone_id)
                .copied()
                .unwrap_or_else(crate::pose::LocalTransform::identity);
            local.rotation = crate::normalize_angle(local.rotation + rotation_delta);
            bone.limit.apply_to_local(&mut local);
            current_pose.set(bone_id, local);
        }

        let new_world = crate::fk_eval_world(skeleton, &current_pose)?;
        let new_effector =
            new_world.get(effector_id).ok_or(CharError::BoneNotFound(effector_id))?;
        let error = (new_effector.translation - target_pos).length();
        final_error = error;

        if error <= 1e-4 {
            converged = true;
            break;
        }
    }

    *pose = current_pose;

    if !converged {
        diagnostics.push(IkDiagnostic::Diverging {
            chain: chain.id,
            iterations: CCD_MAX_ITERATIONS,
        });
    }

    Ok(SolveOutcome {
        success: converged,
        iterations: CCD_MAX_ITERATIONS,
        final_error,
        diagnostics: diagnostics.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pose::{LocalTransform, Pose};
    use crate::schema::default_humanoid_rig;
    use crate::skeleton::Skeleton;
    use crate::BoneId;
    use crate::IkTarget;
    use crate::RigDefinitionId;
    use glam::DVec2;

    #[test]
    fn ccd_reaches_target() {
        let rig = default_humanoid_rig(RigDefinitionId::from_u128(1));
        let skeleton = Skeleton::from_rig(&rig).expect("build skeleton");
        let mut pose = Pose::new();
        let chain = crate::IkChain {
            id: crate::IkChainId::from_u128(3),
            bones: vec![BoneId::from_u128(7), BoneId::from_u128(8), BoneId::from_u128(9)],
            effector: BoneId::from_u128(9),
            solver: crate::SolverKind::Ccd { gain: CCD_GAIN },
            blend: 1.0,
        };
        let target = IkTarget {
            chain: chain.id,
            position: DVec2::new(-17.78, 7.11),
            orientation: 0.0,
            weight: 1.0,
        };
        let mut diagnostics = Vec::new();
        let result =
            solve_ccd(&skeleton, &mut pose, &chain, &target, &mut diagnostics, CCD_GAIN);
        assert!(result.is_ok());
        let outcome = result.unwrap();
        let mut diagnostics = Vec::new();
        let result =
            solve_ccd(&skeleton, &mut pose, &chain, &target, &mut diagnostics, CCD_GAIN);
        assert!(result.is_ok());
    }
}
