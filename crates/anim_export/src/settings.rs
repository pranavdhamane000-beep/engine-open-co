//! Export settings, validation, immutable resolved settings and codec matrix (spec §18.3, §18.4, §18.5).
//!
//! Defines the export configuration schema, validates against project capabilities,
//! and resolves to immutable settings used by the export pipeline.

use crate::error::{ExportError, ExportResult};
use anim_core::time::{FrameRange, FrameRate};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Codec identifier (spec §18.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodecId {
    /// H.264 in MP4 container
    H264Mp4,
    /// H.264 in MOV container
    H264Mov,
    /// ProRes 4444 in MOV container
    ProresMov,
    /// PNG sequence
    PngSequence,
}

/// Quality profile (spec §18.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QualityProfile {
    /// Lossless / visually lossless
    Lossless,
    /// High quality (CRF 18 for H.264, standard for ProRes)
    High,
    /// Medium quality (CRF 23 for H.264)
    Medium,
    /// Preview quality (CRF 28 for H.264)
    Preview,
}

/// Alpha handling mode (spec §18.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportAlphaMode {
    /// Reject alpha (fail if present)
    Reject,
    /// Premultiply alpha into RGB (for opaque output)
    Premultiply,
    /// Keep alpha channel (only for codecs/containers that support it)
    Keep,
    /// Composite over opaque background color
    CompositeOver,
}

/// Audio output mode (spec §18.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioMode {
    /// No audio track
    None,
    /// PCM 16-bit little-endian (WAV)
    PcmS16le,
    /// AAC (lossy)
    Aac,
}

/// On-disk export settings (spec §18.8).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportSettings {
    /// Schema version for forward compatibility
    pub schema_version: u32,
    /// Frame range to export [first, end)
    pub frames: FrameRange,
    /// Frame rate (rational)
    pub fps: FrameRate,
    /// Output width in pixels
    pub width: u32,
    /// Output height in pixels
    pub height: u32,
    /// Video codec
    pub codec: CodecId,
    /// Quality profile
    pub quality: QualityProfile,
    /// Alpha handling
    pub alpha: ExportAlphaMode,
    /// Audio mode
    pub audio: AudioMode,
    /// Output file path
    pub destination: PathBuf,
    /// Overwrite existing file
    pub overwrite: bool,
    /// Optional opaque background color for alpha compositing (RGBA u8)
    pub background_color: Option<[u8; 4]>,
    /// Chunk samples for audio (default 1024)
    pub chunk_samples: u32,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            schema_version: 1,
            frames: FrameRange::new(0, 100).unwrap(),
            fps: anim_core::time::FrameRate::new(24000, 1001).unwrap(),
            width: 1920,
            height: 1080,
            codec: CodecId::H264Mp4,
            quality: QualityProfile::High,
            alpha: ExportAlphaMode::Reject,
            audio: AudioMode::None,
            destination: PathBuf::from("output.mp4"),
            overwrite: false,
            background_color: None,
            chunk_samples: 1024,
        }
    }
}

/// Resolved, immutable export configuration (spec §18.5 step 1).
/// Created by `validate()` and passed to the export pipeline.
#[derive(Debug, Clone)]
pub struct ResolvedExport {
    pub settings: ExportSettings,
    pub profile: CodecProfile,
    pub ffmpeg_path: std::path::PathBuf,
    pub ffprobe_path: std::path::PathBuf,
    pub ffmpeg_version: String,
    pub ffmpeg_hash: String,
    pub ffprobe_version: String,
}

/// Codec profile with explicit FFmpeg arguments (spec §18.6, §18.7).
#[derive(Debug, Clone)]
pub struct CodecProfile {
    pub codec: CodecId,
    pub quality: QualityProfile,
    pub container: ContainerFormat,
    pub video_codec: String,
    pub video_args: Vec<String>,
    pub audio_codec: Option<String>,
    pub audio_args: Vec<String>,
    pub muxer_args: Vec<String>,
    pub file_extension: String,
    pub supports_alpha: bool,
    pub requires_even_dimensions: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerFormat {
    Mp4,
    Mov,
    PngSequence,
}

/// FFmpeg/FFprobe tool paths and versions (spec §18.3, §18.7).
#[derive(Debug, Clone)]
pub struct FFmpegTools {
    pub ffmpeg_path: std::path::PathBuf,
    pub ffprobe_path: std::path::PathBuf,
    pub ffmpeg_version: String,
    pub ffprobe_version: String,
    pub ffmpeg_hash: String,
}

/// Expected output counts for verification (spec §18.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedCounts {
    pub video_frames: u64,
    pub audio_samples: u64,
}

impl ExportSettings {
    /// Validate settings against project and resolve codec profile (spec §18.5 step 1-2).
    pub fn validate(
        &self,
        _project: &anim_project::ProjectDocument,
    ) -> ExportResult<ResolvedExport> {
        // Schema version
        if self.schema_version != 1 {
            return Err(ExportError::InvalidSettings(format!(
                "unsupported schema version: {}",
                self.schema_version
            )));
        }

        // Frame range
        if self.frames.start >= self.frames.end {
            return Err(ExportError::InvalidSettings(
                "empty or reversed frame range".into(),
            ));
        }

        // Dimensions
        if self.width == 0 || self.height == 0 {
            return Err(ExportError::InvalidSettings("zero dimensions".into()));
        }

        // Codec/container validation
        let profile = self.resolve_codec_profile()?;

        // Even dimensions for H.264
        if profile.requires_even_dimensions
            && (!self.width.is_multiple_of(2) || !self.height.is_multiple_of(2))
        {
            return Err(ExportError::OddDimensions {
                width: self.width,
                height: self.height,
            });
        }

        // Alpha handling
        if self.alpha == ExportAlphaMode::Keep && !profile.supports_alpha {
            return Err(ExportError::AlphaNotSupported {
                codec: format!("{:?}", self.codec),
            });
        }

        // Destination
        if self.destination.exists() && !self.overwrite {
            return Err(ExportError::DestinationExists(self.destination.clone()));
        }

        // Parent directory must exist
        if let Some(parent) = self.destination.parent() {
            if !parent.exists() {
                return Err(ExportError::InvalidSettings(format!(
                    "destination parent directory does not exist: {}",
                    parent.display()
                )));
            }
        }

        // Find FFmpeg/FFprobe
        let (ffmpeg_path, ffprobe_path, ffmpeg_version, ffprobe_version, ffmpeg_hash) =
            Self::find_ffmpeg()?;

        Ok(ResolvedExport {
            settings: self.clone(),
            profile,
            ffmpeg_path,
            ffprobe_path,
            ffmpeg_version,
            ffmpeg_hash,
            ffprobe_version,
        })
    }

    fn resolve_codec_profile(&self) -> ExportResult<CodecProfile> {
        use CodecId::*;
        use ExportAlphaMode::*;
        use QualityProfile::*;

        let profile = match (self.codec, self.quality, self.alpha) {
            (H264Mp4, Lossless, Reject) => CodecProfile {
                codec: H264Mp4,
                quality: Lossless,
                container: ContainerFormat::Mp4,
                video_codec: "libx264".into(),
                video_args: vec![
                    "-crf".into(),
                    "0".into(),
                    "-preset".into(),
                    "veryslow".into(),
                    "-pix_fmt".into(),
                    "yuv420p".into(),
                ],
                audio_codec: None,
                audio_args: vec![],
                muxer_args: vec!["-movflags".into(), "+faststart".into()],
                file_extension: "mp4".into(),
                supports_alpha: false,
                requires_even_dimensions: true,
            },
            (H264Mp4, High, Reject) => CodecProfile {
                codec: H264Mp4,
                quality: High,
                container: ContainerFormat::Mp4,
                video_codec: "libx264".into(),
                video_args: vec![
                    "-crf".into(),
                    "18".into(),
                    "-preset".into(),
                    "slow".into(),
                    "-pix_fmt".into(),
                    "yuv420p".into(),
                ],
                audio_codec: None,
                audio_args: vec![],
                muxer_args: vec!["-movflags".into(), "+faststart".into()],
                file_extension: "mp4".into(),
                supports_alpha: false,
                requires_even_dimensions: true,
            },
            (H264Mp4, Medium, Reject) => CodecProfile {
                codec: H264Mp4,
                quality: Medium,
                container: ContainerFormat::Mp4,
                video_codec: "libx264".into(),
                video_args: vec![
                    "-crf".into(),
                    "23".into(),
                    "-preset".into(),
                    "medium".into(),
                    "-pix_fmt".into(),
                    "yuv420p".into(),
                ],
                audio_codec: None,
                audio_args: vec![],
                muxer_args: vec!["-movflags".into(), "+faststart".into()],
                file_extension: "mp4".into(),
                supports_alpha: false,
                requires_even_dimensions: true,
            },
            (H264Mp4, Preview, Reject) => CodecProfile {
                codec: H264Mp4,
                quality: Preview,
                container: ContainerFormat::Mp4,
                video_codec: "libx264".into(),
                video_args: vec![
                    "-crf".into(),
                    "28".into(),
                    "-preset".into(),
                    "fast".into(),
                    "-pix_fmt".into(),
                    "yuv420p".into(),
                ],
                audio_codec: None,
                audio_args: vec![],
                muxer_args: vec!["-movflags".into(), "+faststart".into()],
                file_extension: "mp4".into(),
                supports_alpha: false,
                requires_even_dimensions: true,
            },
            (H264Mov, _, Keep) => CodecProfile {
                codec: H264Mov,
                quality: self.quality,
                container: ContainerFormat::Mov,
                video_codec: "libx264".into(),
                video_args: Self::h264_args(self.quality),
                audio_codec: None,
                audio_args: vec![],
                muxer_args: vec!["-movflags".into(), "+faststart".into()],
                file_extension: "mov".into(),
                supports_alpha: false, // H.264 doesn't truly support alpha
                requires_even_dimensions: true,
            },
            (ProresMov, Lossless, Keep) => CodecProfile {
                codec: ProresMov,
                quality: Lossless,
                container: ContainerFormat::Mov,
                video_codec: "prores_ks".into(),
                video_args: vec![
                    "-profile:v".into(),
                    "4444".into(),
                    "-pix_fmt".into(),
                    "yuva444p10le".into(),
                ],
                audio_codec: None,
                audio_args: vec![],
                muxer_args: vec![],
                file_extension: "mov".into(),
                supports_alpha: true,
                requires_even_dimensions: false,
            },
            (ProresMov, High, Keep) => CodecProfile {
                codec: ProresMov,
                quality: High,
                container: ContainerFormat::Mov,
                video_codec: "prores_ks".into(),
                video_args: vec![
                    "-profile:v".into(),
                    "4444".into(),
                    "-pix_fmt".into(),
                    "yuva444p10le".into(),
                ],
                audio_codec: None,
                audio_args: vec![],
                muxer_args: vec![],
                file_extension: "mov".into(),
                supports_alpha: true,
                requires_even_dimensions: false,
            },
            (PngSequence, _, _) => CodecProfile {
                codec: PngSequence,
                quality: self.quality,
                container: ContainerFormat::PngSequence,
                video_codec: "png".into(),
                video_args: vec!["-pix_fmt".into(), "rgba".into()],
                audio_codec: None,
                audio_args: vec![],
                muxer_args: vec![],
                file_extension: "png".into(),
                supports_alpha: true,
                requires_even_dimensions: false,
            },
            _ => {
                return Err(ExportError::InvalidCodecContainer {
                    codec: format!("{:?}", self.codec),
                    container: format!("{:?}", self.quality),
                })
            }
        };

        Ok(profile)
    }

    fn h264_args(quality: QualityProfile) -> Vec<String> {
        match quality {
            QualityProfile::Lossless => {
                vec!["-crf".into(), "0".into(), "-preset".into(), "veryslow".into()]
            }
            QualityProfile::High => {
                vec!["-crf".into(), "18".into(), "-preset".into(), "slow".into()]
            }
            QualityProfile::Medium => {
                vec!["-crf".into(), "23".into(), "-preset".into(), "medium".into()]
            }
            QualityProfile::Preview => {
                vec!["-crf".into(), "28".into(), "-preset".into(), "fast".into()]
            }
        }
    }

    fn find_ffmpeg(
    ) -> ExportResult<(std::path::PathBuf, std::path::PathBuf, String, String, String)>
    {
        // In production, this reads from config or environment
        // For now, use system PATH
        let ffmpeg_path = which::which("ffmpeg")
            .map_err(|_| ExportError::FfmpegNotFound("ffmpeg not found in PATH".into()))?;
        let ffprobe_path = which::which("ffprobe")
            .map_err(|_| ExportError::FfmpegNotFound("ffprobe not found in PATH".into()))?;

        // Get versions
        let ffmpeg_version = std::process::Command::new(&ffmpeg_path)
            .arg("-version")
            .output()
            .map_err(|e| ExportError::FfmpegProcessFailed(e.to_string()))?
            .stdout;
        let ffmpeg_version = String::from_utf8_lossy(&ffmpeg_version)
            .lines()
            .next()
            .unwrap_or("unknown")
            .to_string();

        let ffprobe_version = std::process::Command::new(&ffprobe_path)
            .arg("-version")
            .output()
            .map_err(|e| ExportError::FfmpegProcessFailed(e.to_string()))?
            .stdout;
        let ffprobe_version = String::from_utf8_lossy(&ffprobe_version)
            .lines()
            .next()
            .unwrap_or("unknown")
            .to_string();

        // Hash the ffmpeg executable
        let ffmpeg_bytes = std::fs::read(&ffmpeg_path)
            .map_err(|e| ExportError::FfmpegProcessFailed(e.to_string()))?;
        let ffmpeg_hash = blake3::hash(&ffmpeg_bytes).to_string();

        Ok((ffmpeg_path, ffprobe_path, ffmpeg_version, ffprobe_version, ffmpeg_hash))
    }
}
