//! Audio error types (spec Section17.11).
//!
//! All errors are typed and explicit. Offline errors abort before
//! publication. Preview device errors stop audio, increment generation
//! and leave editing available. No error may silently shift clip
//! offsets or replace a source with silence.

use anim_core::id::TrackId;
use anim_core::CoreError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("invalid sample rate {0}: must be > 0")]
    InvalidSampleRate(u32),
    #[error("invalid channel count {0}: must be 1 or 2")]
    InvalidChannelCount(u16),
    #[error("empty or reversed frame range")]
    EmptyFrameRange,
    #[error("frame rate {0} is zero or negative")]
    InvalidFrameRate(String),
    #[error("export sample offset overflow: {0}")]
    SampleOffsetOverflow(String),
    #[error("negative export sample offset: {0}")]
    NegativeExportOffset(String),
    #[error("clip {0}: source length is zero but looped=true")]
    ZeroLengthLoopedClip(TrackId),
    #[error("clip {0}: gain {1} is not finite")]
    NonFiniteGain(TrackId, f64),
    #[error("clip {0}: fade samples exceed source length")]
    FadeExceedsSource(TrackId),
    #[error("unsupported channel layout: {0} channels")]
    UnsupportedChannelLayout(u16),
    #[error("missing source asset: {0}")]
    MissingAsset(String),
    #[error("asset changed: expected hash {expected}, got {actual}")]
    AssetHashMismatch { expected: String, actual: String },
    #[error("decoder failure: {0}")]
    DecodeFailure(String),
    #[error("resampler failure: {0}")]
    ResampleFailure(String),
    #[error("device loss: {0}")]
    DeviceLoss(String),
    #[error("output stream failure: {0}")]
    OutputStreamFailure(String),
    #[error("WAV write failure: {0}")]
    WavWriteFailure(String),
    #[error("buffer capacity exceeded: requested {requested}, capacity {capacity}")]
    BufferCapacityExceeded { requested: usize, capacity: usize },
}

#[derive(Debug, Error)]
pub enum PreviewError {
    #[error("cpal stream error: {0}")]
    StreamError(String),
    #[error("device not found: {0}")]
    DeviceNotFound(String),
    #[error("format not supported: {0}")]
    FormatNotSupported(String),
    #[error("ring buffer error: {0}")]
    RingBufferError(String),
    #[error("transport error: {0}")]
    TransportError(String),
}

impl From<CoreError> for AudioError {
    fn from(e: CoreError) -> Self {
        AudioError::DecodeFailure(e.to_string())
    }
}

impl From<std::io::Error> for AudioError {
    fn from(e: std::io::Error) -> Self {
        AudioError::DecodeFailure(e.to_string())
    }
}

impl From<symphonia::core::errors::Error> for AudioError {
    fn from(e: symphonia::core::errors::Error) -> Self {
        AudioError::DecodeFailure(e.to_string())
    }
}

impl From<rubato::ResampleError> for AudioError {
    fn from(e: rubato::ResampleError) -> Self {
        AudioError::ResampleFailure(e.to_string())
    }
}

pub type AudioResult<T> = Result<T, AudioError>;
pub type PreviewResult<T> = Result<T, PreviewError>;
