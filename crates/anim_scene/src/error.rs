//! Scene-layer errors — the exact §13.11 set plus typed structural cases.

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SceneError {
    #[error("scene {0} not found in document")]
    MissingScene(String),
    #[error("hierarchy cycle detected at node {0}")]
    HierarchyCycle(String),
    #[error("node {0}: parent {1} missing from scene")]
    MissingParent(String, String),
    #[error("duplicate node id {0}")]
    DuplicateNode(String),
    #[error("invalid property keys: {0}")]
    InvalidPropertyKeys(String),
    #[error("invalid easing for property '{0}' at {1}")]
    InvalidEasing(String, String),
    #[error("time overflow: {0}")]
    TimeOverflow(String),
    #[error("invalid numeric value: {0}")]
    InvalidNumericValue(String),
    #[error("event order overflow: {0}")]
    EventOrderOverflow(String),
    /// A Cut marker names no camera node in the scene (camera-cut rule,
    /// documented in ADR-0008; typed rather than skipped per §13.11).
    #[error("camera cut references unknown camera '{0}'")]
    UnknownCamera(String),
}

impl From<anim_core::CoreError> for SceneError {
    fn from(e: anim_core::CoreError) -> Self {
        match e {
            anim_core::CoreError::ArithmeticOverflow
            | anim_core::CoreError::SubframeOutOfRange(_) => {
                SceneError::TimeOverflow(e.to_string())
            }
            other => SceneError::TimeOverflow(other.to_string()),
        }
    }
}

pub type SceneResult<T> = Result<T, SceneError>;
