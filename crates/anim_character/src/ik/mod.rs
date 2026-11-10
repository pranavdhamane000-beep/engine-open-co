//! Inverse Kinematics solvers (spec §27 Algorithms).
//!
//! Implements analytic two-bone IK, FABRIK, CCD, and Jacobian DLS.
//! All solvers enforce limits inside iteration, never after.

pub mod ccd;
pub mod fabrik;
pub mod jacobian;
pub mod switch;
pub mod two_bone;

pub use ccd::solve_ccd;
pub use fabrik::solve_fabrik;
pub use jacobian::solve_jacobian_dls;
pub use switch::{blend_ik_fk, lerp_angle};
pub use two_bone::solve_two_bone;
// Types are re-exported from crate root (defined in types.rs)

use crate::error::CharResult;
use crate::pose::Pose;
use crate::skeleton::Skeleton;
use crate::{IkChain, IkTarget, SolveOutcome, SolverKind};

/// Dispatch solver based on chain configuration.
pub fn solve_ik(
    skeleton: &Skeleton,
    pose: &mut Pose,
    chain: &IkChain,
    target: &IkTarget,
    diagnostics: &mut Vec<crate::IkDiagnostic>,
) -> CharResult<SolveOutcome> {
    match chain.solver {
        SolverKind::TwoBone { pole } => {
            solve_two_bone(skeleton, pose, chain, target, diagnostics, pole)
        }
        SolverKind::Fabrik { tolerance } => {
            solve_fabrik(skeleton, pose, chain, target, diagnostics, tolerance)
        }
        SolverKind::Ccd { gain } => {
            solve_ccd(skeleton, pose, chain, target, diagnostics, gain)
        }
        SolverKind::JacobianDls { lambda } => {
            solve_jacobian_dls(skeleton, pose, chain, target, diagnostics, lambda)
        }
    }
}

/// Full-body solve entry point (spec §27 Internal logic).
pub fn solve_full_body(
    _state: &crate::CharacterState,
    _at: anim_core::FramePos,
) -> CharResult<crate::ResolvedCharacter> {
    todo!("solve_full_body requires anim_sim integration")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_compiles() {
        // Just verify the module structure compiles
    }
}
