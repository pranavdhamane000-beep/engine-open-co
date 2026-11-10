//! Character rig error types (spec §27).
//!
//! Hard structural failures abort project load with exact IDs.
//! Recoverable IK failures return diagnostics for structured handling.

use crate::{BoneId, IkChainId, RigDefinitionId, SocketId};
use thiserror::Error;

/// Result type for character operations.
pub type CharResult<T> = Result<T, CharError>;

/// Character rig error categories (spec §27 Error handling).
#[derive(Debug, Error)]
pub enum CharError {
    /// Duplicate bone IDs in rig definition.
    #[error("duplicate bone ID: {0}")]
    DuplicateBoneId(BoneId),

    /// Bone references missing parent.
    #[error("missing parent: bone {bone} references parent {parent}")]
    MissingParent { bone: BoneId, parent: BoneId },

    /// Cycle detected in rig hierarchy.
    #[error("cycle detected in rig hierarchy involving bone {0}")]
    CycleDetected(BoneId),

    /// Unsupported schema version.
    #[error("unsupported rig schema version: {0}")]
    UnsupportedSchemaVersion(u32),

    /// Rig definition not found.
    #[error("rig definition not found: {0}")]
    RigNotFound(RigDefinitionId),

    /// Bone not found in skeleton.
    #[error("bone not found in skeleton: {0}")]
    BoneNotFound(BoneId),

    /// IK chain not found.
    #[error("IK chain not found: {0}")]
    IkChainNotFound(IkChainId),

    /// IK target unreachable — clamped to nearest reach.
    #[error("IK target unreachable for chain {chain}: requested {requested:.6}, max reach {max_reach:.6}")]
    IkUnreachable { chain: IkChainId, requested: f64, max_reach: f64 },

    /// IK singular — increased damping or fallback.
    #[error("IK singular for chain {chain}: {detail}")]
    IkSingular { chain: IkChainId, detail: String },

    /// Joint limit conflict.
    #[error(
        "limit conflict on bone {bone}: requested {requested:.6}, clamped to {clamped:.6}"
    )]
    LimitConflict { bone: BoneId, requested: f64, clamped: f64 },

    /// IK diverged — reverted to last bounded pose.
    #[error("IK diverged for chain {chain} after {iterations} iterations: reverted to last bounded pose")]
    Diverging { chain: IkChainId, iterations: usize },

    /// Non-finite value detected.
    #[error("non-finite value in {context}: {value}")]
    NonFinite { context: String, value: f64 },

    /// Invalid rig definition structure.
    #[error("invalid rig definition: {0}")]
    InvalidRigDefinition(String),

    /// Socket not found.
    #[error("socket not found: {0}")]
    SocketNotFound(SocketId),

    /// Contact lock conflict.
    #[error("contact lock conflict: {0}")]
    ContactLockConflict(String),

    /// Root motion conflict with contact lock.
    #[error("root motion conflicts with active contact lock on bone {0}")]
    RootMotionConflict(BoneId),

    /// Schema migration failure.
    #[error("schema migration failed: {0}")]
    MigrationFailed(String),

    /// Solver iteration limit exceeded.
    #[error("solver iteration limit exceeded: {0}")]
    IterationLimitExceeded(String),

    /// Invalid pole source for two-bone IK.
    #[error("invalid pole source for two-bone IK: {0}")]
    InvalidPoleSource(String),

    /// Blend weight out of range [0, 1].
    #[error("blend weight out of range [0, 1]: {0}")]
    BlendWeightOutOfRange(f64),

    /// Pose mask invalid for bone.
    #[error("pose mask invalid for bone {0}")]
    PoseMaskInvalid(BoneId),
}

#[cfg(test)]
mod tests {
    use super::*;
    use anim_core::NodeId;

    #[test]
    fn error_messages_contain_ids() {
        let e = CharError::DuplicateBoneId(BoneId::from_u128(42));
        assert!(e.to_string().contains("duplicate bone ID"));

        let e = CharError::IkUnreachable {
            chain: IkChainId::from_u128(1),
            requested: 10.0,
            max_reach: 8.0,
        };
        assert!(e.to_string().contains("IK target unreachable"));
        assert!(e.to_string().contains("10"));
        assert!(e.to_string().contains("8"));
    }
}
