//! Export error types (spec §18.11).
//!
//! Stable categories for validation, asset, simulation, GPU, memory,
//! encoder, mux, verification, I/O, cancellation and publication.
//! Broken pipes include FFmpeg status and bounded diagnostics.
//! Timeouts kill and reap the process. Disk-full, short readback,
//! malformed output and verification failures prevent publication.
//! An existing destination without overwrite fails before rendering.
//! Errors never trigger a lower-quality retry or an unverified codec fallback.

use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExportError {
    #[error("invalid export settings: {0}")]
    InvalidSettings(String),
    #[error("missing or changed asset: {0}")]
    MissingAsset(String),
    #[error("asset hash mismatch: expected {expected}, got {actual}")]
    AssetHashMismatch { expected: String, actual: String },
    #[error("simulation preparation failed: {0}")]
    SimulationFailed(String),
    #[error("GPU render failure: {0}")]
    GpuRenderFailed(String),
    #[error("memory budget exceeded: {0}")]
    MemoryBudgetExceeded(String),
    #[error("readback failure: {0}")]
    ReadbackFailed(String),
    #[error("FFmpeg not found or invalid: {0}")]
    FfmpegNotFound(String),
    #[error("FFmpeg executable hash mismatch: expected {expected}, got {actual}")]
    FfmpegHashMismatch { expected: String, actual: String },
    #[error("FFmpeg capability missing: {0}")]
    FfmpegCapabilityMissing(String),
    #[error("FFmpeg process failed: {0}")]
    FfmpegProcessFailed(String),
    #[error("FFmpeg stderr: {0}")]
    FfmpegStderr(String),
    #[error("encoder start failed: {0}")]
    EncoderStartFailed(String),
    #[error("encoder write failed: {0}")]
    EncoderWriteFailed(String),
    #[error("mux exit failure: {0}")]
    MuxExitFailure(String),
    #[error("decode verification failed: {0}")]
    DecodeVerifyFailed(String),
    #[error("frame count mismatch: expected {expected}, got {actual}")]
    FrameCountMismatch { expected: u64, actual: u64 },
    #[error("audio sample count mismatch: expected {expected}, got {actual}")]
    AudioSampleCountMismatch { expected: u64, actual: u64 },
    #[error("disk full: {0}")]
    DiskFull(String),
    #[error("short readback: expected {expected} bytes, got {actual}")]
    ShortReadback { expected: usize, actual: usize },
    #[error("malformed output: {0}")]
    MalformedOutput(String),
    #[error("disk full during write: {0}")]
    DiskFullWrite(String),
    #[error("cancellation requested")]
    Cancelled,
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("device lost: {0}")]
    DeviceLost(String),
    #[error("publication failure: {0}")]
    PublicationFailed(String),
    #[error("destination exists and overwrite=false: {0}")]
    DestinationExists(PathBuf),
    #[error("atomic replace failed: {0}")]
    AtomicReplaceFailed(String),
    #[error("invalid codec for container: codec={codec}, container={container}")]
    InvalidCodecContainer { codec: String, container: String },
    #[error("odd dimensions not supported for codec: {width}x{height}")]
    OddDimensions { width: u32, height: u32 },
    #[error("alpha not supported for codec without opaque background: {codec}")]
    AlphaNotSupported { codec: String },
    #[error("verification failed: {0}")]
    VerificationFailed(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type ExportResult<T> = Result<T, ExportError>;
