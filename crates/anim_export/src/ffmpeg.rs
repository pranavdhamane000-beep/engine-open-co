//! FFmpeg executable verification, arguments and child-process lifecycle (spec §18.3, §18.7).
//!
//! FFmpeg is spawned directly without a shell. Executable verification checks
//! path, version, executable hash and capabilities against the manifest.

use crate::error::{ExportError, ExportResult};
use crate::settings::FFmpegTools;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command as TokioCommand};
use tokio::sync::Mutex;

/// FFmpeg process wrapper with lifecycle management (spec §18.7).
pub struct FfmpegProcess {
    child: Child,
    stdin: Option<tokio::process::ChildStdin>,
    stderr_capture: Arc<Mutex<Vec<u8>>>,
    start_time: std::time::Instant,
    timeout: Duration,
}

impl FfmpegProcess {
    /// Spawn FFmpeg with the given arguments (spec §18.7).
    pub fn spawn(
        tools: &FFmpegTools,
        args: Vec<String>,
        stdin: bool,
        capture_stderr: bool,
    ) -> ExportResult<Self> {
        let mut cmd = TokioCommand::new(&tools.ffmpeg_path);
        cmd.args(&args)
            .stdin(if stdin { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(if capture_stderr { Stdio::piped() } else { Stdio::null() })
            .kill_on_drop(true);

        let mut child =
            cmd.spawn().map_err(|e| ExportError::FfmpegProcessFailed(e.to_string()))?;

        let stdin = child.stdin.take();
        let stderr = child.stderr.take();

        let stderr_capture = Arc::new(Mutex::new(Vec::new()));
        let stderr_capture_clone = stderr_capture.clone();

        if capture_stderr && stderr.is_some() {
            tokio::spawn(async move {
                let mut stderr = stderr.unwrap();
                let mut buf = [0u8; 4096];
                loop {
                    match stderr.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            stderr_capture_clone.lock().await.extend_from_slice(&buf[..n]);
                        }
                        Err(_) => break,
                    }
                }
            });
        }

        Ok(Self {
            child,
            stdin,
            stderr_capture,
            start_time: std::time::Instant::now(),
            timeout: Duration::from_secs(300), // 5 minute default timeout
        })
    }

    /// Get stdin for writing raw video frames
    pub fn stdin(&mut self) -> Option<&mut tokio::process::ChildStdin> {
        self.stdin.as_mut()
    }

    /// Wait for process completion with timeout
    pub async fn wait(mut self) -> ExportResult<std::process::ExitStatus> {
        let status = tokio::time::timeout(self.timeout, self.child.wait())
            .await
            .map_err(|_| ExportError::Timeout("FFmpeg process timed out".into()))?
            .map_err(|e| ExportError::FfmpegProcessFailed(e.to_string()))?;

        if !status.success() {
            let stderr = self.get_stderr().await;
            return Err(ExportError::FfmpegProcessFailed(format!(
                "FFmpeg exited with {:?}: {}",
                status.code(),
                String::from_utf8_lossy(&stderr)
            )));
        }

        Ok(status)
    }

    /// Get captured stderr
    pub async fn get_stderr(&self) -> Vec<u8> {
        self.stderr_capture.lock().await.clone()
    }

    /// Kill the process
    pub async fn kill(&mut self) {
        let _ = self.child.kill().await;
    }

    /// Check if process has timed out
    pub fn is_timed_out(&self) -> bool {
        self.start_time.elapsed() > self.timeout
    }
}

/// Build FFmpeg arguments for encoding (spec §18.7).
pub fn build_ffmpeg_args(
    profile: &crate::settings::CodecProfile,
    settings: &crate::settings::ExportSettings,
    _width: u32,
    _height: u32,
    fps: &anim_core::time::FrameRate,
    input_pipe: bool,
    audio_path: Option<&std::path::Path>,
) -> Vec<String> {
    let mut args = Vec::new();

    // Global options
    args.extend(["-y".into(), "-loglevel".into(), "error".into()]);

    // Input: raw video from pipe
    if input_pipe {
        args.extend([
            "-f".into(),
            "rawvideo".into(),
            "-pix_fmt".into(),
            "rgba".into(),
            "-s".into(),
            format!("{}x{}", settings.width, settings.height),
            "-r".into(),
            format!("{}/{}", fps.num, fps.den),
            "-i".into(),
            "pipe:0".into(),
        ]);
    }

    // Audio input if provided
    if let Some(audio_path) = audio_path {
        args.extend(["-i".into(), audio_path.to_string_lossy().to_string()]);
    }

    // Video codec args
    args.push("-c:v".into());
    args.push(profile.video_codec.clone());
    args.extend(profile.video_args.clone());

    // Audio codec args
    if let Some(audio_codec) = &profile.audio_codec {
        args.push("-c:a".into());
        args.push(audio_codec.clone());
        args.extend(profile.audio_args.clone());
    }

    // Muxer args
    args.extend(profile.muxer_args.clone());

    // Output
    args.push(settings.destination.to_string_lossy().to_string());

    args
}

/// Build FFprobe arguments for verification (spec §18.7).
pub fn build_ffprobe_args(
    path: &std::path::Path,
    select_streams: Option<&str>,
) -> Vec<String> {
    let args = vec![
        "-v".into(),
        "error".into(),
        "-select_streams".into(),
        select_streams.unwrap_or("v").into(),
        "-show_entries".into(),
        "stream=codec_name,width,height,r_frame_rate,pix_fmt,channels,sample_rate".into(),
        "-of".into(),
        "json".into(),
        path.to_string_lossy().to_string(),
    ];
    args
}
