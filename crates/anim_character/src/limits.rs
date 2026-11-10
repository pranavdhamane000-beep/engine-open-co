//! Joint limits for character rig (spec §27 Data structures).

use crate::error::{CharError, CharResult};
use serde::{Deserialize, Serialize};

/// Joint limit types.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum JointLimit {
    /// No limits (free joint).
    Free,
    /// Revolute (hinge) joint with min/max angle in radians.
    Revolute { min: f64, max: f64 },
    /// Ball joint with cone half-angle in radians.
    Ball { cone_half_angle: f64 },
    /// Fixed joint (no movement).
    Fixed,
}

impl JointLimit {
    pub fn validate(&self) -> CharResult<()> {
        match self {
            JointLimit::Free => Ok(()),
            JointLimit::Fixed => Ok(()),
            JointLimit::Revolute { min, max } => {
                if !min.is_finite() || !max.is_finite() {
                    return Err(CharError::NonFinite {
                        context: "Revolute limit".into(),
                        value: f64::NAN,
                    });
                }
                if min >= max {
                    return Err(CharError::InvalidRigDefinition(format!(
                        "Revolute min ({min}) must be < max ({max})"
                    )));
                }
                Ok(())
            }
            JointLimit::Ball { cone_half_angle } => {
                let angle = *cone_half_angle;
                if !angle.is_finite() {
                    return Err(CharError::NonFinite {
                        context: "Ball cone angle".into(),
                        value: f64::NAN,
                    });
                }
                if angle <= 0.0 || angle >= std::f64::consts::PI {
                    return Err(CharError::InvalidRigDefinition(format!(
                        "Ball cone_half_angle must be in (0, PI), got {angle}"
                    )));
                }
                Ok(())
            }
        }
    }

    pub fn clamp_rotation(&self, rotation: f64) -> f64 {
        match self {
            JointLimit::Free => rotation,
            JointLimit::Fixed => 0.0,
            JointLimit::Revolute { min, max } => rotation.clamp(*min, *max),
            JointLimit::Ball { cone_half_angle } => {
                if rotation.abs() <= *cone_half_angle {
                    rotation
                } else {
                    rotation.signum() * *cone_half_angle
                }
            }
        }
    }

    pub fn contains_rotation(&self, rotation: f64) -> bool {
        match self {
            JointLimit::Free => true,
            JointLimit::Fixed => rotation.abs() < 1e-10,
            JointLimit::Revolute { min, max } => rotation >= *min && rotation <= *max,
            JointLimit::Ball { cone_half_angle } => {
                rotation.abs() <= *cone_half_angle + 1e-10
            }
        }
    }

    pub fn apply_to_local(&self, local: &mut crate::pose::LocalTransform) {
        local.rotation = self.clamp_rotation(local.rotation);
    }
}

/// Apply all joint limits to a pose.
pub fn apply_limits(
    skeleton: &crate::skeleton::Skeleton,
    pose: &mut crate::pose::Pose,
) -> CharResult<()> {
    for bone in skeleton.bones() {
        if let Some(local) = pose.get(bone.id) {
            let mut clamped = *local;
            bone.limit.apply_to_local(&mut clamped);
            pose.set(bone.id, clamped);
        }
    }
    Ok(())
}
