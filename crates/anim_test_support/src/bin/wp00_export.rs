//! WP00 export probe â€” spec 9.5 step 8, 9.9 `run_export_probe`.
//!
//! Proves on this machine:
//!   1. a tiny RAW video (16 deterministic RGB24 frames) + deterministic WAV
//!      mux through FFmpeg (`libx264 -qp 0`, PCM audio in Matroska)
//!   2. FFprobe on the result declares EXACTLY one video + one audio stream
//!      with the expected frame metadata (decode-side stream counting,
//!      spec 9.7: "decode the result and count streams")
//!   3. the decoded video is **byte-identical** to the generated pattern
//!      (lossless path verified end to end)
//!   4. the decoded audio returns the exact source sample count
//!      (PCM mux â†’ WAV decode, zero delay).
//!
//! Arg-array exec, bounded timeout, drained stderr (spec 9.4/9.6).

use hound::WavSpec;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const W: u32 = 64;
const H: u32 = 64;
const FRAMES: u32 = 16;
const AUDIO_SR: u32 = 44_100;
const AUDIO_SECONDS: u32 = 1;
const MAX_CHILD_SECONDS: u64 = 20;

#[derive(Serialize)]
struct ExportProbeReport {
    schema_version: u32,
    capability: String,
    success: bool,
    streams_found: Option<(usize, usize)>, // declared (video, audio) in output
    video_frames_declared: Option<u64>,
    video_bytes_equal: Option<bool>,
    audio_samples_decoded: Option<u64>,
    diagnostics: Vec<String>,
}

/// Print the full detailed report and exit with the prescribed status.
fn report_exit(report: &ExportProbeReport) -> ! {
    println!("{}", serde_json::to_string_pretty(report).unwrap_or_default());
    std::process::exit(if report.success { 0 } else { 1 });
}

fn load_tool_path(key: &str) -> Result<PathBuf, String> {
    let raw = std::fs::read_to_string("config/default.toml")
        .map_err(|e| format!("config read: {e}"))?;
    let prefix = format!("{key} = \"");
    for line in raw.lines() {
        let t = line.trim_start();
        if t.starts_with(&prefix) {
            let rest = &t[prefix.len()..];
            let direct = PathBuf::from(rest.trim_end_matches('"'));
            return if direct.exists() {
                Ok(direct)
            } else {
                Err(format!("{key} points to missing binary: {}", direct.display()))
            };
        }
    }
    Err(format!("config key `{key}` missing"))
}

fn run_bounded(
    exe: &Path,
    args: &[&str],
    dir: &Path,
) -> Result<(i32, String, String), String> {
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().map_err(|e| format!("spawn: {e}"))?;
    let done = Arc::new(AtomicBool::new(false));
    let pid = child.id();
    {
        let done = done.clone();
        std::thread::spawn(move || {
            for _ in 0..MAX_CHILD_SECONDS * 10 {
                if done.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            let _ = Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/F", "/T"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        });
    }
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let out_t = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(p) = stdout_pipe.as_mut() {
            use std::io::Read;
            let _ = p.read_to_string(&mut s);
        }
        s
    });
    let err_t = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(p) = stderr_pipe.as_mut() {
            use std::io::Read;
            let _ = p.read_to_string(&mut s);
        }
        s
    });
    let stdout = out_t.join().unwrap_or_default();
    let stderr = err_t.join().unwrap_or_default();
    let status = child.wait().map_err(|e| format!("wait: {e}"))?;
    done.store(true, Ordering::Relaxed);
    match status.code() {
        Some(code) => Ok((code, stdout, stderr)),
        None => Err(format!("timeout: force-terminated pid {pid}")),
    }
}

/// Deterministic frame pattern: red = x ramp, green = y ramp, blue = frame
/// index marker. Blue channel proves frame ORDER at decode time.
fn frame_bytes(f: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((W * H * 3) as usize);
    for y in 0..H {
        for x in 0..W {
            v.push((x * 255 / W) as u8);
            v.push((y * 255 / H) as u8);
            v.push((f * 16).clamp(0, 255) as u8);
        }
    }
    v
}

fn packet() -> Vec<u8> {
    let mut raw = Vec::new();
    for f in 0..FRAMES {
        raw.extend_from_slice(&frame_bytes(f));
    }
    raw
}

fn main() {
    let mut report = ExportProbeReport {
        schema_version: 1,
        capability: "export_rawvideo_wav_mux_verify".into(),
        success: false,
        streams_found: None,
        video_frames_declared: None,
        video_bytes_equal: None,
        audio_samples_decoded: None,
        diagnostics: Vec::new(),
    };

    let ffmpeg = match load_tool_path("ffmpeg_local") {
        Ok(p) => p,
        Err(e) => {
            report.diagnostics.push(e);
            report_exit(&report);
        }
    };
    let ffprobe = match load_tool_path("ffprobe_local") {
        Ok(p) => p,
        Err(e) => {
            report.diagnostics.push(e);
            report_exit(&report);
        }
    };

    let dir = std::env::temp_dir().join("wp00_export_probe");
    let _ = std::fs::create_dir_all(&dir);
    for f in
        ["frames.raw", "audio.wav", "out.mkv", "back.raw", "audio_back.wav", "probe.json"]
    {
        let _ = std::fs::remove_file(dir.join(f));
    }

    // 1. deterministic inputs
    let raw = packet();
    let expected_video_bytes = raw.len() as u64;
    std::fs::write(dir.join("frames.raw"), &raw).map_err(|e| e.to_string()).unwrap_or_else(
        |e| {
            report.diagnostics.push(format!("write raw: {e}"));
            report_exit(&report);
        },
    );
    let wav_spec = WavSpec {
        channels: 1,
        sample_rate: AUDIO_SR,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = match hound::WavWriter::create(dir.join("audio.wav"), wav_spec) {
        Ok(w) => w,
        Err(e) => {
            report.diagnostics.push(format!("wav create: {e}"));
            report_exit(&report);
        }
    };
    for i in 0..(AUDIO_SR as usize * AUDIO_SECONDS as usize) {
        let t = i as f32 / AUDIO_SR as f32;
        let _ = writer.write_sample(
            ((t * 660.0 * 2.0 * std::f32::consts::PI).sin() * 30_000.0) as i16,
        );
    }
    if let Err(e) = writer.finalize() {
        report.diagnostics.push(format!("wav finalize: {e}"));
        report_exit(&report);
    }

    // 2. encode + mux: lossless H.264 video, PCM audio, Matroska container
    let (code, _o, stderr) = match run_bounded(
        &ffmpeg,
        &[
            "-y",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-s",
            "64x64",
            "-framerate",
            "24",
            "-i",
            "frames.raw",
            "-i",
            "audio.wav",
            "-c:v",
            "libx264rgb",
            "-qp",
            "0",
            "-preset",
            "ultrafast",
            "-c:a",
            "pcm_s16le",
            "out.mkv",
        ],
        &dir,
    ) {
        Ok(r) => r,
        Err(e) => {
            report.diagnostics.push(e);
            report_exit(&report);
        }
    };
    if code != 0 {
        report.diagnostics.push(format!("mux encode failed: exit {code}: {stderr}"));
        report_exit(&report);
    }

    // 3. ffprobe stream count + frame metadata (decode-side counting)
    let (code, json, stderr) = match run_bounded(
        &ffprobe,
        &[
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            "-count_frames",
            "out.mkv",
        ],
        &dir,
    ) {
        Ok(r) => r,
        Err(e) => {
            report.diagnostics.push(e);
            report_exit(&report);
        }
    };
    if code != 0 {
        report.diagnostics.push(format!("ffprobe exit {code}: {stderr}"));
        report_exit(&report);
    }
    let parsed: serde_json::Value = match serde_json::from_str(&json) {
        Ok(v) => v,
        Err(e) => {
            report.diagnostics.push(format!("malformed ffprobe JSON: {e}"));
            report_exit(&report);
        }
    };
    let streams = match parsed.get("streams").and_then(|v| v.as_array()) {
        Some(s) => s,
        None => {
            report.diagnostics.push("missing streams in ffprobe JSON".into());
            report_exit(&report);
        }
    };
    let video = streams.iter().filter(|s| s["codec_type"] == "video").count();
    let audio = streams.iter().filter(|s| s["codec_type"] == "audio").count();
    report.streams_found = Some((video, audio));
    if video != 1 || audio != 1 {
        report.diagnostics.push(format!(
            "stream count wrong: expected 1 video + 1 audio, got {video}/{audio}"
        ));
        report_exit(&report);
    }
    report.video_frames_declared = streams[0]
        .get("nb_read_frames")
        .or_else(|| streams[0].get("nb_frames"))
        // ffmpeg emits the value as a JSON string, not a number
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok());
    if report.video_frames_declared != Some(FRAMES as u64) {
        report.diagnostics.push(format!("declared frames != {FRAMES}"));
    }

    // 4. decode back to raw RGB24 and compare byte-exact
    let (code, _o, stderr) = match run_bounded(
        &ffmpeg,
        &[
            "-y",
            "-loglevel",
            "error",
            "-i",
            "out.mkv",
            "-map",
            "0:v:0",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "back.raw",
        ],
        &dir,
    ) {
        Ok(r) => r,
        Err(e) => {
            report.diagnostics.push(e);
            report_exit(&report);
        }
    };
    if code != 0 {
        report.diagnostics.push(format!("video decode failed: {stderr}"));
        report_exit(&report);
    }
    let decoded = std::fs::read(dir.join("back.raw")).unwrap_or_default();
    let equal = decoded.len() == raw.len() && decoded == raw;
    report.video_bytes_equal = Some(equal);
    if !equal {
        report.diagnostics.push(format!(
            "decoded video bytes differ (got {} B, expected {expected_video_bytes} B)",
            decoded.len()
        ));
    }

    // 5. audio decode back: exact sample count (PCM â†’ PCM is delay-free)
    let (code, _o, stderr) = match run_bounded(
        &ffmpeg,
        &[
            "-y",
            "-loglevel",
            "error",
            "-i",
            "out.mkv",
            "-map",
            "0:a:0",
            "-f",
            "wav",
            "-c:a",
            "pcm_s16le",
            "audio_back.wav",
        ],
        &dir,
    ) {
        Ok(r) => r,
        Err(e) => {
            report.diagnostics.push(e);
            report_exit(&report);
        }
    };
    if code != 0 {
        report.diagnostics.push(format!("audio decode failed: {stderr}"));
        report_exit(&report);
    }
    match hound::WavReader::open(dir.join("audio_back.wav")) {
        Ok(mut rdr) => {
            let spec = rdr.spec();
            let n = rdr.samples::<i16>().count() as u64 / spec.channels.max(1) as u64;
            report.audio_samples_decoded = Some(n);
            if n != (AUDIO_SR * AUDIO_SECONDS) as u64 {
                report.diagnostics.push(format!(
                    "audio samples {} != expected {}",
                    n,
                    AUDIO_SR * AUDIO_SECONDS
                ));
            }
        }
        Err(e) => {
            report.diagnostics.push(format!("audio_back.wav read: {e}"));
            report_exit(&report);
        }
    }

    report.success = report.streams_found == Some((1, 1))
        && report.video_frames_declared == Some(FRAMES as u64)
        && report.video_bytes_equal == Some(true)
        && report.audio_samples_decoded == Some((AUDIO_SR * AUDIO_SECONDS) as u64);
    report_exit(&report);
}
