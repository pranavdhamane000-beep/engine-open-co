//! Project/asset/settings/environment job manifest (spec §18.3, §18.6).
//!
//! The manifest captures the complete export context for reproducibility
//! and verification. It includes project hash, asset manifest hash,
//! settings hash, and FFmpeg environment details.

use crate::error::{ExportError, ExportResult};
use crate::settings::ExportSettings;
use anim_project::ProjectDocument;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

/// Complete job manifest for an export job (spec §18.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportManifest {
    /// Manifest schema version
    pub schema_version: u32,
    /// Project content hash (hex-encoded)
    pub project_hash: String,
    /// Asset manifest hash (all referenced assets)
    pub asset_manifest_hash: String,
    /// Export settings hash
    pub settings_hash: String,
    /// FFmpeg executable hash
    pub ffmpeg_hash: String,
    /// FFmpeg version string
    pub ffmpeg_version: String,
    /// FFprobe version string
    pub ffprobe_version: String,
    /// Export settings (validated)
    pub settings: ExportSettings,
    /// Resolved codec profile name
    pub profile_name: String,
    /// Export start timestamp
    pub started_at: SystemTime,
    /// Export working directory (temp paths)
    pub working_dir: PathBuf,
    /// Output destination
    pub destination: PathBuf,
}

impl ExportManifest {
    pub fn new(
        project: &ProjectDocument,
        asset_manifest_hash: &str,
        resolved: &crate::settings::ResolvedExport,
        working_dir: PathBuf,
    ) -> ExportResult<Self> {
        // Hash the project document using serde serialization
        let project_bytes = serde_json::to_vec(project)
            .map_err(|e| ExportError::InvalidSettings(e.to_string()))?;
        let project_hash = blake3::hash(&project_bytes).to_string();

        // Serialize settings and hash
        let settings_bytes = serde_json::to_vec(&resolved.settings)
            .map_err(|e| ExportError::InvalidSettings(e.to_string()))?;
        let settings_hash = blake3::hash(&settings_bytes).to_string();

        // Profile name
        let profile_name = format!(
            "{:?}_{:?}_{:?}",
            resolved.settings.codec, resolved.settings.quality, resolved.settings.alpha
        );

        Ok(Self {
            schema_version: 1,
            project_hash,
            asset_manifest_hash: asset_manifest_hash.to_string(),
            settings_hash,
            ffmpeg_hash: resolved.ffmpeg_hash.to_string(),
            ffmpeg_version: resolved.ffmpeg_version.clone(),
            ffprobe_version: resolved.ffprobe_version.clone(),
            settings: resolved.settings.clone(),
            profile_name,
            started_at: SystemTime::now(),
            working_dir,
            destination: resolved.settings.destination.clone(),
        })
    }

    /// Write manifest to file
    pub fn write(&self, path: &std::path::Path) -> Result<(), std::io::Error> {
        let json = serde_json::to_vec_pretty(self)?;
        std::fs::write(path, json)
    }

    /// Read manifest from file
    pub fn read(path: &std::path::Path) -> Result<Self, std::io::Error> {
        let json = std::fs::read_to_string(path)?;
        serde_json::from_str(&json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

/// Expected output counts for verification (spec §18.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectedCounts {
    pub video_frames: u64,
    pub audio_samples: u64,
}

/// Export report with hashes and metadata (spec §18.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportReport {
    pub video_frames: u64,
    pub audio_samples: u64,
    pub project_hash: String,
    pub asset_manifest_hash: String,
    pub settings_hash: String,
    pub output_hash: String,
    pub ffmpeg_version: String,
}
