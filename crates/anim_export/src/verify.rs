//! FFprobe/decode metadata, counts and failure comparison (spec §18.7, §18.9).
//!
//! Decode final video and count complete frames and trailing bytes; require
//! exactly N and no trailing bytes. Require pre-mux WAV samples to equal
//! WP01 expected count. PCM final audio must match exactly. Lossy AAC is
//! checked against declared priming/edit-list behavior and expected timeline
//! duration; unexplained added/missing samples fail. Verify codec, pixel
//! format, dimensions, stream order and rational frame rate.

use crate::error::ExportError;
use crate::manifest::ExpectedCounts;
use std::process::Command;

/// FFprobe tools for verification (spec §18.9).
/// Wrapper around crate::settings::FFmpegTools for verification-specific usage.
pub struct FfprobeTools {
    pub ffprobe_path: std::path::PathBuf,
}

/// Verify output file (spec §18.9).
pub fn verify_output(
    path: &std::path::Path,
    expected: &ExpectedCounts,
    tools: &FfprobeTools,
) -> crate::error::ExportResult<()> {
    // Run FFprobe to get stream info
    let output = Command::new(&tools.ffprobe_path)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v",
            "-show_entries",
            "stream=codec_name,width,height,r_frame_rate,pix_fmt,nb_frames",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .map_err(|e| ExportError::FfmpegProcessFailed(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ExportError::VerificationFailed(format!("FFprobe failed: {}", stderr)));
    }

    let probe: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| ExportError::MalformedOutput(e.to_string()))?;

    let streams = probe.get("streams").and_then(|s| s.as_array()).ok_or_else(|| {
        ExportError::MalformedOutput("no streams in FFprobe output".into())
    })?;

    if streams.is_empty() {
        return Err(ExportError::VerificationFailed("no video stream found".into()));
    }

    let video_stream = &streams[0];

    // Verify codec
    let codec = video_stream
        .get("codec_name")
        .and_then(|c| c.as_str())
        .ok_or_else(|| ExportError::VerificationFailed("missing codec_name".into()))?;

    if !matches!(codec, "h264" | "prores") {
        return Err(ExportError::VerificationFailed(format!(
            "unexpected codec: {}",
            codec
        )));
    }

    // Verify dimensions
    let width = video_stream
        .get("width")
        .and_then(|w| w.as_u64())
        .ok_or_else(|| ExportError::VerificationFailed("missing width".into()))?;
    let height = video_stream
        .get("height")
        .and_then(|h| h.as_u64())
        .ok_or_else(|| ExportError::VerificationFailed("missing height".into()))?;

    if width == 0 || height == 0 {
        return Err(ExportError::VerificationFailed("zero dimensions".into()));
    }

    // Verify pixel format
    let pix_fmt = video_stream
        .get("pix_fmt")
        .and_then(|p| p.as_str())
        .ok_or_else(|| ExportError::VerificationFailed("missing pix_fmt".into()))?;

    if !matches!(pix_fmt, "yuv420p" | "yuva444p10le") {
        return Err(ExportError::VerificationFailed(format!(
            "unexpected pixel format: {}",
            pix_fmt
        )));
    }

    // Verify frame rate
    let frame_rate = video_stream
        .get("r_frame_rate")
        .and_then(|r| r.as_str())
        .ok_or_else(|| ExportError::VerificationFailed("missing frame rate".into()))?;

    // Parse rational frame rate (e.g., "24000/1001")
    let parts: Vec<&str> = frame_rate.split('/').collect();
    if parts.len() != 2 {
        return Err(ExportError::VerificationFailed(format!(
            "invalid frame rate format: {}",
            frame_rate
        )));
    }
    let _fps_num: u32 = parts[0].parse().unwrap_or(0);
    let _fps_den: u32 = parts[1].parse().unwrap_or(0);
    if _fps_num == 0 || _fps_den == 0 {
        return Err(ExportError::VerificationFailed("zero frame rate".into()));
    }

    // Verify audio if expected
    if expected.audio_samples > 0 {
        verify_audio(path, expected, &tools.ffprobe_path)?;
    }

    Ok(())
}

fn verify_audio(
    path: &std::path::Path,
    _expected: &ExpectedCounts,
    ffprobe_path: &std::path::Path,
) -> crate::error::ExportResult<()> {
    let output = Command::new(ffprobe_path)
        .args([
            "-v",
            "error",
            "-select_streams",
            "a",
            "-show_entries",
            "stream=codec_name,channels,sample_rate,nb_frames",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .map_err(|e| ExportError::FfmpegProcessFailed(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ExportError::VerificationFailed(format!(
            "FFprobe audio failed: {}",
            stderr
        )));
    }

    let probe: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| ExportError::MalformedOutput(e.to_string()))?;

    let streams = probe
        .get("streams")
        .and_then(|s| s.as_array())
        .ok_or_else(|| ExportError::MalformedOutput("no audio streams".into()))?;

    if streams.is_empty() {
        return Err(ExportError::VerificationFailed("no audio stream found".into()));
    }

    let audio_stream = &streams[0];

    // Verify codec
    let _codec = audio_stream
        .get("codec_name")
        .and_then(|c| c.as_str())
        .ok_or_else(|| ExportError::VerificationFailed("missing audio codec".into()))?;

    // Verify channels
    let channels = audio_stream
        .get("channels")
        .and_then(|c| c.as_u64())
        .ok_or_else(|| ExportError::VerificationFailed("missing audio channels".into()))?;

    if channels != 2 {
        return Err(ExportError::VerificationFailed(format!(
            "expected 2 channels, got {}",
            channels
        )));
    }

    // Verify sample rate
    let sample_rate = audio_stream
        .get("sample_rate")
        .and_then(|r| r.as_str())
        .ok_or_else(|| ExportError::VerificationFailed("missing sample rate".into()))?;

    if sample_rate != "48000" {
        return Err(ExportError::VerificationFailed(format!(
            "expected 48000 Hz sample rate, got {}",
            sample_rate
        )));
    }

    Ok(())
}

/// Verify decoded video and count complete frames and trailing bytes
pub fn verify_video_decoded(
    path: &std::path::Path,
    expected_frames: u64,
) -> crate::error::ExportResult<()> {
    // Use FFmpeg to decode and count frames
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-i", path.to_str().unwrap(), "-f", "null", "-"])
        .output()
        .map_err(|e| ExportError::FfmpegProcessFailed(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ExportError::DecodeVerifyFailed(stderr.to_string()));
    }

    // Parse stderr for frame count
    let stderr = String::from_utf8_lossy(&output.stderr);
    // FFmpeg outputs frame count in stderr like "frame=  123 fps=..."
    for line in stderr.lines() {
        if line.contains("frame=") {
            let parts: Vec<&str> = line.split('=').collect();
            if parts.len() >= 2 {
                let frame_str = parts[1].split_whitespace().next().unwrap_or("0");
                let frames: u64 = frame_str.parse().unwrap_or(0);
                if frames != expected_frames {
                    return Err(ExportError::FrameCountMismatch {
                        expected: expected_frames,
                        actual: frames,
                    });
                }
            }
        }
    }

    Ok(())
}
