//! Core types for anim_character (spec §27 Data structures).
//!
//! Uses glam::Vec2 for 2D vectors. Types that contain Vec2 cannot derive
//! Serialize/Deserialize unless glam has serde feature enabled.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Bone identifier (newtype wrapper around anim_core::NodeId).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BoneId(pub anim_core::NodeId);

impl BoneId {
    pub fn from_u128(v: u128) -> Self {
        Self(anim_core::NodeId(uuid::Uuid::from_u128(v)))
    }
}

impl Default for BoneId {
    fn default() -> Self {
        Self::from_u128(0)
    }
}

impl From<anim_core::NodeId> for BoneId {
    fn from(n: anim_core::NodeId) -> Self {
        Self(n)
    }
}

impl fmt::Display for BoneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Rig definition identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RigDefinitionId(pub uuid::Uuid);

impl RigDefinitionId {
    pub fn from_u128(v: u128) -> Self {
        Self(uuid::Uuid::from_u128(v))
    }
}

impl Default for RigDefinitionId {
    fn default() -> Self {
        Self::from_u128(0)
    }
}

impl fmt::Display for RigDefinitionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// IK chain identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct IkChainId(pub uuid::Uuid);

impl IkChainId {
    pub fn from_u128(v: u128) -> Self {
        Self(uuid::Uuid::from_u128(v))
    }
}

impl Default for IkChainId {
    fn default() -> Self {
        Self::from_u128(0)
    }
}

impl fmt::Display for IkChainId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Socket identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SocketId(pub uuid::Uuid);

impl SocketId {
    pub fn from_u128(v: u128) -> Self {
        Self(uuid::Uuid::from_u128(v))
    }
}

impl Default for SocketId {
    fn default() -> Self {
        Self::from_u128(0)
    }
}

impl fmt::Display for SocketId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// IK chain configuration (spec §27 Data structures).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IkChain {
    pub id: IkChainId,
    pub bones: Vec<BoneId>,
    pub effector: BoneId,
    pub solver: SolverKind,
    pub blend: f64,
}

impl IkChain {
    pub fn new(
        id: IkChainId,
        bones: Vec<BoneId>,
        effector: BoneId,
        solver: SolverKind,
    ) -> Self {
        Self { id, bones, effector, solver, blend: 1.0 }
    }
}

/// Solver algorithm variants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SolverKind {
    TwoBone { pole: crate::ik::two_bone::PoleSource },
    Fabrik { tolerance: f64 },
    Ccd { gain: f64 },
    JacobianDls { lambda: f64 },
}

impl Default for SolverKind {
    fn default() -> Self {
        Self::Fabrik { tolerance: 1e-4 }
    }
}

/// IK target specification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IkTarget {
    pub chain: IkChainId,
    pub position: DVec2,
    pub orientation: f64,
    pub weight: f64,
}

impl IkTarget {
    pub fn new(chain: IkChainId, position: DVec2) -> Self {
        Self { chain, position, orientation: 0.0, weight: 1.0 }
    }
}

/// Solver outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolveOutcome {
    pub success: bool,
    pub iterations: usize,
    pub final_error: f64,
    pub diagnostics: Vec<IkDiagnostic>,
}

/// IK solver diagnostics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum IkDiagnostic {
    Unreachable { chain: IkChainId, requested: f64, max_reach: f64 },
    Singular { chain: IkChainId, detail: String },
    LimitConflict { bone: BoneId, requested: f64, clamped: f64 },
    Diverging { chain: IkChainId, iterations: usize },
}

/// Character state for simulation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterState {
    pub skeleton: crate::skeleton::Skeleton,
    pub current_pose: crate::pose::Pose,
    pub active_chains: BTreeMap<IkChainId, IkChain>,
    pub targets: BTreeMap<IkChainId, Vec<IkTarget>>,
    pub contact_locks: BTreeMap<BoneId, crate::motion::ContactLock>,
    pub root_transform: crate::motion::RootTransform,
    pub sockets: BTreeMap<SocketId, crate::socket::Socket>,
}

impl CharacterState {
    pub fn new(skeleton: crate::skeleton::Skeleton) -> Self {
        Self {
            skeleton,
            current_pose: crate::pose::Pose::new(),
            active_chains: BTreeMap::new(),
            targets: BTreeMap::new(),
            contact_locks: BTreeMap::new(),
            root_transform: crate::motion::RootTransform::identity(),
            sockets: BTreeMap::new(),
        }
    }
}

/// Resolved character data for scene snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedCharacter {
    pub skeleton_id: RigDefinitionId,
    pub pose: crate::pose::Pose,
    pub world_pose: crate::pose::WorldPose,
    pub root_transform: crate::motion::RootTransform,
    pub contacts: BTreeMap<BoneId, crate::motion::ContactLock>,
    pub sockets: BTreeMap<SocketId, crate::pose::WorldTransform>,
    pub frame: anim_core::FramePos,
    pub diagnostics: Vec<IkDiagnostic>,
}

/// Re-export commonly used types.
pub use anim_core::{FramePos, Rational};
pub use glam::DVec2;
