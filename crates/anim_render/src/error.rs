//! WP06 error taxonomy (spec §15.11): every failure is typed and LOUD.
//! No silent backend, transfer-function or quality fallback is allowed.
//!
//! Entity fields are formatted descriptions (NodeId display form) so the
//! error type stays independent of engine internals.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("missing required texture format or usage: {0}")]
    MissingRequiredFormat(String),

    /// Zero-extent resize is REJECTED and the last valid targets are kept
    /// intact (§15.8: targets are recreated atomically, never destroyed
    /// by a bad resize).
    #[error("zero extent {0}x{1}x{2} rejected; last valid targets retained")]
    ZeroExtent(u32, u32, u32),

    #[error("non-finite transform or draw data on entity {0} (draw_index {1})")]
    NaNTransform(String, u32),

    #[error("non-finite or invalid camera/viewport data: {0}")]
    NonfiniteCamera(String),

    #[error("alpha cutoff {0} invalid; must be finite and within (0.0, 1.0]")]
    InvalidAlphaCutoff(f32),

    #[error("non-finite view depth on entity {0}")]
    NonfiniteViewDepth(String),

    #[error("duplicate sort identity (entity {0}, draw_index {1})")]
    DuplicateSortIdentity(String, u32),

    #[error("node opacity {0} outside [0, 1]")]
    InvalidOpacity(f64),

    #[error("malformed snapshot: {0}")]
    InvalidSnapshot(String),

    #[error("invalid render graph configuration: {0}")]
    InvalidGraph(String),

    /// Surface lost/outdated → the receiver reconfigures (§15.11).
    #[error("surface lost; reconfiguration required")]
    SurfaceLost,
    #[error("surface outdated; reconfiguration required")]
    SurfaceOutdated,

    #[error("device lost; history and GPU targets invalidated; clean re-initialization required")]
    DeviceLost,

    #[error("out of memory: {0}")]
    OutOfMemory(String),

    #[error("export failure: {0}")]
    ExportError(String),
}

pub type RenderResult<T> = Result<T, RenderError>;
