//! Editor error set (spec 19.11).
//!
//! No failure path crashes the event loop, creates duplicate commands or
//! produces a hidden project edit. Malformed pointer data is REJECTED
//! (typed error, stroke state preserved); a transient command-submission
//! failure keeps the pending stroke retryable under the SAME command id.

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EditorError {
    #[error("ink disabled: {0} (mouse editing remains available)")]
    InkDisabled(String),
    #[error("malformed pointer data: {0} (event rejected, no state change)")]
    InvalidPointerData(String),
    #[error(
        "maximum stroke point count exceeded — stroke cancelled visibly, nothing committed"
    )]
    MaxPointsExceeded { limit: u32, received: u32 },
    #[error("no active stroke")]
    NoActiveStroke,
    #[error("operation not valid in stroke state {state:?}")]
    WrongState { state: super::ink::StrokeState },
    #[error("unsupported tool: {0}")]
    UnsupportedTool(String),
    #[error("device lost during operation: {0}")]
    DeviceLost(String),
    #[error("export rejected: {0}")]
    ExportRejected(String),
    #[error("transport: {0}")]
    Transport(String),
    #[error("command rejected: {0}")]
    CommandRejected(String),
    #[error("transient commit failure (stroke retryable with the same id): {0}")]
    RetryableCommit(String),
}
