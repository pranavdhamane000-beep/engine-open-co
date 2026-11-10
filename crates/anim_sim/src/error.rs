//! Typed simulator errors — the exact §14.11 set (plus typed sub-cases)
//! as a source of DO-HOW errors; never clamps, never substitutes.

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Error)]
pub enum SimError {
    #[error("invalid simulation rate: {0}")]
    InvalidRate(String),
    #[error("tick overflow converting {0}")]
    TickOverflow(String),
    #[error("negative tick or replay target: {0}")]
    NegativeTick(String),
    #[error("negative age at spawn tick {tick}: age {age}")]
    NegativeAge { tick: i64, age: String },
    #[error("duplicated stable id: {0}")]
    DuplicateId(String),
    #[error("nonfinite simulation state at tick {tick}: {what}")]
    NonfiniteState { tick: i64, what: String },
    #[error("invalid solver iteration count: {0}")]
    InvalidSolverCount(u16),
    #[error("event targets unknown node: {0}")]
    UnknownEventTarget(String),
    #[error("malformed checkpoint: {0}")]
    MalformedCheckpoint(String),
    #[error("checkpoint hash mismatch at tick {0}")]
    CheckpointHashMismatch(i64),
    #[error("unsupported random algorithm version: {0}")]
    UnsupportedRandomAlgorithm(u32),
    #[error("invalid random lane {0} (must be 0..=3)")]
    InvalidLane(u8),
    #[error("replay budget exhausted at tick {tick}; raise the explicit limit")]
    ReplayBudgetExhausted { tick: i64 },
    #[error("missing simulation context input: {0}")]
    MissingInput(String),
    #[error("random stream version mismatch: requested stream version {0}")]
    StreamVersionMismatch(u32),
    #[error("checkpoint cache corrupt at tick {0}: {1}")]
    CacheCorrupt(i64, String),
}

pub type SimResult<T> = Result<T, SimError>;
