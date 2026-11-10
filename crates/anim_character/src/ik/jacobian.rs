#![allow(clippy::all)]
//! Jacobian Damped Least Squares (DLS) solver.

use crate::error::{CharError, CharResult};
use crate::pose::Pose;
use crate::skeleton::Skeleton;
use crate::solver_constants::*;
use crate::IkDiagnostic;
use crate::SolveOutcome;

#[allow(clippy::indexing_slicing)]
pub fn solve_jacobian_dls(
    skeleton: &Skeleton,
    pose: &mut Pose,
    chain: &crate::IkChain,
    target: &crate::IkTarget,
    diagnostics: &mut Vec<IkDiagnostic>,
    lambda: f64,
) -> CharResult<SolveOutcome> {
    let n = chain.bones.len();
    if n < 2 {
        return Err(CharError::InvalidRigDefinition(
            "Jacobian DLS requires at least 2 bones".into(),
        ));
    }

    let effector_id = chain.effector;
    let target_pos = target.position;
    let target_orient = target.orientation;

    let mut damping = lambda.max(JACOBIAN_BASE_LAMBDA);
    let mut current_pose = pose.clone();
    let mut final_error = 0.0;
    let mut converged = false;

    for _iter in 0..JACOBIAN_MAX_ITERATIONS {
        let world = crate::fk_eval_world(skeleton, &current_pose)?;
        let effector_world =
            world.get(effector_id).ok_or(CharError::BoneNotFound(effector_id))?;

        let pos_error = target_pos - effector_world.translation;
        let orient_error = crate::normalize_angle(target_orient - effector_world.rotation);

        let error_norm = (pos_error.x * pos_error.x
            + pos_error.y * pos_error.y
            + orient_error * orient_error)
            .sqrt();
        final_error = error_norm;

        if error_norm <= 1e-4 {
            converged = true;
            break;
        }

        let mut jacobian = vec![vec![0.0; n]; 3];

        let eps = 1e-4;
        for i in 0..n {
            let bone_id = chain.bones[i];
            let original = current_pose
                .get(bone_id)
                .copied()
                .unwrap_or_else(crate::pose::LocalTransform::identity);

            let mut pose_plus = current_pose.clone();
            let mut local_plus = original;
            local_plus.rotation += eps;
            pose_plus.set(bone_id, local_plus);
            let world_plus = crate::fk_eval_world(skeleton, &pose_plus)?;
            let eff_plus =
                world_plus.get(effector_id).ok_or(CharError::BoneNotFound(effector_id))?;
            let pos_plus = eff_plus.translation;
            let orient_plus = eff_plus.rotation;

            let mut pose_minus = current_pose.clone();
            let mut local_minus = original;
            local_minus.rotation -= eps;
            pose_minus.set(bone_id, local_minus);
            let world_minus = crate::fk_eval_world(skeleton, &pose_minus)?;
            let eff_minus =
                world_minus.get(effector_id).ok_or(CharError::BoneNotFound(effector_id))?;
            let pos_minus = eff_minus.translation;
            let orient_minus = eff_minus.rotation;

            jacobian[0][i] = (pos_plus.x - pos_minus.x) / (2.0 * eps);
            jacobian[1][i] = (pos_plus.y - pos_minus.y) / (2.0 * eps);
            jacobian[2][i] =
                crate::normalize_angle(orient_plus - orient_minus) / (2.0 * eps);
        }

        let mut jjt = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                let mut sum = 0.0;
                for k in 0..n {
                    sum += jacobian[i][k] * jacobian[j][k];
                }
                jjt[i][j] = sum + if i == j { damping * damping } else { 0.0 };
            }
        }

        let error_vec = [pos_error.x, pos_error.y, orient_error];
        let delta_err = solve_3x3(&jjt, &error_vec);

        if delta_err.is_none() {
            damping *= 2.0;
            if damping > 1.0 {
                diagnostics.push(IkDiagnostic::Singular {
                    chain: chain.id,
                    detail: "Jacobian singular, max damping reached".into(),
                });
                break;
            }
            continue;
        }

        let delta_err = delta_err.unwrap();

        let mut delta_theta = vec![0.0; n];
        for i in 0..n {
            let mut sum = 0.0;
            for j in 0..3 {
                sum += jacobian[j][i] * delta_err[j];
            }
            delta_theta[i] = sum.clamp(-JACOBIAN_MAX_DELTA, JACOBIAN_MAX_DELTA);
        }

        let mut new_pose = current_pose.clone();
        let mut limit_hit = false;

        for i in 0..n {
            let bone_id = chain.bones[i];
            let mut local = current_pose
                .get(bone_id)
                .copied()
                .unwrap_or_else(crate::pose::LocalTransform::identity);
            local.rotation = crate::normalize_angle(local.rotation + delta_theta[i]);

            let bone = skeleton.bone(bone_id)?;
            bone.limit.apply_to_local(&mut local);

            if (local.rotation
                - current_pose.get(bone_id).map(|l| l.rotation).unwrap_or(0.0))
            .abs()
                > 1e-10
            {
                limit_hit = true;
            }
            new_pose.set(bone_id, local);
        }

        if limit_hit {
            diagnostics.push(IkDiagnostic::LimitConflict {
                bone: chain.bones[0],
                requested: 0.0,
                clamped: 0.0,
            });
        }

        current_pose = new_pose;
    }

    *pose = current_pose;

    if !converged {
        diagnostics.push(IkDiagnostic::Diverging {
            chain: chain.id,
            iterations: JACOBIAN_MAX_ITERATIONS,
        });
    }

    Ok(SolveOutcome {
        success: converged,
        iterations: JACOBIAN_MAX_ITERATIONS,
        final_error,
        diagnostics: diagnostics.clone(),
    })
}

fn solve_3x3(a: &[[f64; 3]; 3], b: &[f64; 3]) -> Option<[f64; 3]> {
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
        - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
        + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);

    if det.abs() < JACOBIAN_SINGULAR_EPS {
        return None;
    }

    let mut x = [0.0; 3];
    for i in 0..3 {
        let mut ai = *a;
        for row in 0..3 {
            ai[row][i] = b[row];
        }
        let det_i = ai[0][0] * (ai[1][1] * ai[2][2] - ai[1][2] * ai[2][1])
            - ai[0][1] * (ai[1][0] * ai[2][2] - ai[1][2] * ai[2][0])
            + ai[0][2] * (ai[1][0] * ai[2][1] - ai[1][1] * ai[2][0]);
        x[i] = det_i / det;
    }

    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solve_3x3_identity() {
        let a = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let b = [1.0, 2.0, 3.0];
        let x = solve_3x3(&a, &b).expect("solve");
        assert!((x[0] - 1.0).abs() < 1e-10);
        assert!((x[1] - 2.0).abs() < 1e-10);
        assert!((x[2] - 3.0).abs() < 1e-10);
    }

    #[test]
    fn solve_3x3_singular_returns_none() {
        let a = [[1.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        let b = [1.0, 2.0, 3.0];
        assert!(solve_3x3(&a, &b).is_none());
    }
}
