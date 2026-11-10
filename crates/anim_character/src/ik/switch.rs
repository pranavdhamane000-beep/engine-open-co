//! IK/FK switching and solver dispatch.

use crate::pose::LocalTransform;
use crate::skeleton::Skeleton;
use crate::IkChain;

pub fn blend_ik_fk(
    _skeleton: &Skeleton,
    fk_pose: &crate::pose::Pose,
    ik_pose: &crate::pose::Pose,
    chain: &IkChain,
    alpha: f64,
) -> crate::CharResult<crate::pose::Pose> {
    if alpha <= 0.0 {
        return Ok(fk_pose.clone());
    }
    if alpha >= 1.0 {
        return Ok(ik_pose.clone());
    }

    let mut blended = fk_pose.clone();
    for &bone_id in &chain.bones {
        let fk_local =
            fk_pose.get(bone_id).copied().unwrap_or_else(LocalTransform::identity);
        let ik_local =
            ik_pose.get(bone_id).copied().unwrap_or_else(LocalTransform::identity);

        let rot = lerp_angle(fk_local.rotation, ik_local.rotation, alpha);
        let tx = fk_local.translation.x * (1.0 - alpha) + ik_local.translation.x * alpha;
        let ty = fk_local.translation.y * (1.0 - alpha) + ik_local.translation.y * alpha;
        let sx = fk_local.scale.x * (1.0 - alpha) + ik_local.scale.x * alpha;
        let sy = fk_local.scale.y * (1.0 - alpha) + ik_local.scale.y * alpha;

        let mut blended_local = fk_local;
        blended_local.rotation = crate::normalize_angle(rot);
        blended_local.translation.x = tx;
        blended_local.translation.y = ty;
        blended_local.scale.x = sx;
        blended_local.scale.y = sy;

        blended.set(bone_id, blended_local);
    }
    Ok(blended)
}

pub fn lerp_angle(a: f64, b: f64, t: f64) -> f64 {
    let diff = crate::normalize_angle(b - a);
    crate::normalize_angle(a + diff * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BoneId;
    use crate::IkChainId;
    use crate::IkTarget;
    use crate::SolverKind;
    use glam::DVec2;

    #[test]
    fn ik_chain_default_blend() {
        let chain = IkChain::new(
            IkChainId::from_u128(1),
            vec![],
            BoneId::from_u128(1),
            SolverKind::default(),
        );
        assert_eq!(chain.blend, 1.0);
    }

    #[test]
    fn ik_target_default_weight() {
        let target = IkTarget::new(IkChainId::from_u128(1), DVec2::ZERO);
        assert_eq!(target.weight, 1.0);
    }
}
