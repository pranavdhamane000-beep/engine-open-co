//! anim_character — Full-Body IK/FK and Character Articulation (spec §27, WP12).
//!
//! The runtime character-rig subsystem that evaluates a hierarchical skeleton
//! through deterministic forward kinematics, analytic two-bone IK, iterative
//! FABRIK/CCD options and a damped-least-squares Jacobian option. Adds joint
//! limits, IK/FK switching, root motion, contact locking, weighted pose
//! blending, full-body stabilization, and hand/face-ready socket contracts.

#![deny(clippy::all)]
#![allow(clippy::multiple_crate_versions)]
#![allow(clippy::indexing_slicing)]

pub mod error;
pub mod ik;
pub mod limits;
pub mod motion;
pub mod pose;
pub mod schema;
pub mod skeleton;
pub mod socket;
pub mod types;

pub use error::{CharError, CharResult};
pub use ik::{blend_ik_fk, lerp_angle, solve_full_body, solve_ik};
pub use limits::JointLimit;
pub use motion::{ContactLock, LockRelease, RootDelta, RootTransform};
pub use pose::{fk_eval_world, LocalTransform, Pose, WorldPose, WorldTransform};
pub use schema::RigDefinition;
pub use skeleton::{Bone, Skeleton};
pub use socket::{Socket, SocketKind};
pub use types::{
    BoneId, CharacterState, IkChain, IkChainId, IkDiagnostic, IkTarget, ResolvedCharacter,
    RigDefinitionId, SocketId, SolveOutcome, SolverKind,
};

/// Deterministic angle normalization to [-π, π).
pub fn normalize_angle(mut angle: f64) -> f64 {
    angle %= 2.0 * std::f64::consts::PI;
    if angle < -std::f64::consts::PI {
        angle += 2.0 * std::f64::consts::PI;
    } else if angle > std::f64::consts::PI {
        angle -= 2.0 * std::f64::consts::PI;
    }
    angle
}

/// Safe inverse cosine with clamping to [-1, 1] domain.
pub fn safe_acos(x: f64) -> f64 {
    x.clamp(-1.0, 1.0).acos()
}

/// Safe inverse sine with clamping to [-1, 1] domain.
pub fn safe_asin(x: f64) -> f64 {
    x.clamp(-1.0, 1.0).asin()
}

/// Safe inverse tangent (y, x) → [-π, π].
pub fn safe_atan2(y: f64, x: f64) -> f64 {
    y.atan2(x)
}

/// Fixed iteration constants for iterative solvers (spec §27).
pub mod solver_constants {
    pub const FABRIK_MAX_ITERATIONS: usize = 20;
    pub const FABRIK_TOLERANCE: f64 = 1e-8;
    pub const CCD_MAX_ITERATIONS: usize = 500;
    pub const CCD_GAIN: f64 = 1.0;
    pub const JACOBIAN_MAX_ITERATIONS: usize = 15;
    pub const JACOBIAN_BASE_LAMBDA: f64 = 0.01;
    pub const JACOBIAN_SINGULAR_EPS: f64 = 1e-6;
    pub const JACOBIAN_MAX_DELTA: f64 = 0.5;
    pub const TWO_BONE_EPS: f64 = 1e-6;
    pub const STABILIZATION_MAX_ITERATIONS: usize = 10;
    pub const STABILIZATION_TOLERANCE: f64 = 1e-6;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angle_normalization_works() {
        assert!((normalize_angle(0.0) - 0.0).abs() < 1e-10);
        assert!(
            (normalize_angle(std::f64::consts::PI) - std::f64::consts::PI).abs() < 1e-10
        );
        assert!(
            (normalize_angle(-std::f64::consts::PI) - (-std::f64::consts::PI)).abs()
                < 1e-10
        );
    }

    #[test]
    fn safe_trig_clamps() {
        assert!((safe_acos(2.0) - 0.0).abs() < 1e-10);
        assert!((safe_acos(-2.0) - std::f64::consts::PI).abs() < 1e-10);
        assert!((safe_asin(2.0) - std::f64::consts::PI / 2.0).abs() < 1e-10);
        assert!((safe_asin(-2.0) + std::f64::consts::PI / 2.0).abs() < 1e-10);
    }

    #[test]
    fn solver_constants_are_reasonable() {
        assert!(solver_constants::FABRIK_MAX_ITERATIONS > 0);
        assert!(solver_constants::CCD_MAX_ITERATIONS > 0);
        assert!(solver_constants::JACOBIAN_MAX_ITERATIONS > 0);
    }
}
