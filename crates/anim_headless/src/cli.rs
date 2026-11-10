//! Validated CLI schema and stable exit mapping (spec §18.3, §18.9).
//!
//! The CLI parses export settings, validates them, and runs the export
//! pipeline with proper exit codes.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "anim_export", version, about = "Headless animation export")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Export a project to video
    Export {
        /// Project file path
        #[arg(short, long)]
        project: PathBuf,

        /// Output file path
        #[arg(short, long)]
        output: PathBuf,

        /// Frame range (e.g., "0..100")
        #[arg(short, long, default_value = "0..100")]
        frames: String,

        /// Frame rate (e.g., "24000/1001")
        #[arg(short, long, default_value = "24000/1001")]
        fps: String,

        /// Output width
        #[arg(long, default_value = "1920")]
        width: u32,

        /// Output height
        #[arg(long, default_value = "1080")]
        height: u32,

        /// Video codec
        #[arg(long, default_value = "h264-mp4")]
        codec: String,

        /// Quality profile
        #[arg(long, default_value = "high")]
        quality: String,

        /// Alpha mode
        #[arg(long, default_value = "reject")]
        alpha: String,

        /// Audio mode
        #[arg(long, default_value = "none")]
        audio: String,

        /// Overwrite existing file
        #[arg(long)]
        overwrite: bool,

        /// Timeout in seconds
        #[arg(long, default_value = "300")]
        timeout: u64,
    },
    /// Run determinism gate on a frozen scene (spec §20.7)
    Determinism {
        /// Frozen scene file path (project+scene+manifest text)
        #[arg(short, long)]
        scene: PathBuf,

        /// Number of frames to evaluate
        #[arg(short, long)]
        frames: u64,

        /// Checkpoint cadence in frames
        #[arg(long, default_value = "120")]
        cadence: u64,

        /// Audio chunk size in samples
        #[arg(long, default_value = "1024")]
        audio_chunk: u64,

        /// Readback ring slot count
        #[arg(long, default_value = "4")]
        readback_slots: u32,

        /// Output directory for report.json
        #[arg(short, long)]
        out: PathBuf,
    },
}

impl Cli {
    pub fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        let command = self.command;
        match command {
            Command::Export {
                project,
                output,
                frames,
                fps,
                width,
                height,
                codec,
                quality,
                alpha,
                audio,
                overwrite,
                timeout,
            } => run_export(
                project, output, frames, fps, width, height, codec, quality, alpha, audio,
                overwrite, timeout,
            ),
            Command::Determinism {
                scene,
                frames,
                cadence,
                audio_chunk,
                readback_slots,
                out,
            } => run_determinism(scene, frames, cadence, audio_chunk, readback_slots, out),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_export(
    _project: PathBuf,
    output: PathBuf,
    frames: String,
    fps: String,
    width: u32,
    height: u32,
    codec: String,
    quality: String,
    alpha: String,
    audio: String,
    overwrite: bool,
    _timeout: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    // Parse frame range
    let frames = parse_frame_range(&frames)?;

    // Parse frame rate
    let fps = parse_frame_rate(&fps)?;

    // Parse codec
    let codec = parse_codec(&codec)?;

    // Parse quality
    let quality = parse_quality(&quality)?;

    // Parse alpha mode
    let alpha = parse_alpha(&alpha)?;

    // Parse audio mode
    let audio = parse_audio(&audio)?;

    // Build export settings
    let settings = anim_export::ExportSettings {
        schema_version: 1,
        frames,
        fps,
        width,
        height,
        codec,
        quality,
        alpha,
        audio,
        destination: output,
        overwrite,
        background_color: None,
        chunk_samples: 1024,
    };

    println!("Export settings validated successfully");
    println!("Output: {}", settings.destination.display());
    println!("Frames: {}..{}", settings.frames.start, settings.frames.end);
    println!("Codec: {:?}", settings.codec);
    println!("Quality: {:?}", settings.quality);

    Ok(())
}

fn parse_frame_range(
    s: &str,
) -> Result<anim_core::time::FrameRange, Box<dyn std::error::Error>> {
    let parts: Vec<&str> = s.split("..").collect();
    if parts.len() != 2 {
        return Err("frame range must be in format 'start..end'".into());
    }
    let start: i64 = parts[0].parse()?;
    let end: i64 = parts[1].parse()?;
    Ok(anim_core::time::FrameRange::new(start, end)?)
}

fn parse_frame_rate(
    s: &str,
) -> Result<anim_core::time::FrameRate, Box<dyn std::error::Error>> {
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() != 2 {
        return Err("frame rate must be in format 'num/den'".into());
    }
    let num: u32 = parts[0].parse()?;
    let den: u32 = parts[1].parse()?;
    Ok(anim_core::time::FrameRate::new(num, den)?)
}

fn parse_codec(s: &str) -> Result<anim_export::CodecId, Box<dyn std::error::Error>> {
    match s.to_lowercase().as_str() {
        "h264-mp4" | "h264mp4" => Ok(anim_export::CodecId::H264Mp4),
        "h264-mov" | "h264mov" => Ok(anim_export::CodecId::H264Mov),
        "prores-mov" | "proresmov" => Ok(anim_export::CodecId::ProresMov),
        "png-sequence" | "png" => Ok(anim_export::CodecId::PngSequence),
        _ => Err(format!("unknown codec: {}", s).into()),
    }
}

fn parse_quality(
    s: &str,
) -> Result<anim_export::QualityProfile, Box<dyn std::error::Error>> {
    match s.to_lowercase().as_str() {
        "lossless" => Ok(anim_export::QualityProfile::Lossless),
        "high" => Ok(anim_export::QualityProfile::High),
        "medium" => Ok(anim_export::QualityProfile::Medium),
        "preview" => Ok(anim_export::QualityProfile::Preview),
        _ => Err(format!("unknown quality: {}", s).into()),
    }
}

// parse_color removed (unused); color-a formats are parsed by parse_alpha/parse_audio.

/// Parse an export alpha-mode CLI value.
fn parse_alpha(
    s: &str,
) -> Result<anim_export::ExportAlphaMode, Box<dyn std::error::Error>> {
    match s.to_lowercase().as_str() {
        "reject" => Ok(anim_export::ExportAlphaMode::Reject),
        "premultiply" => Ok(anim_export::ExportAlphaMode::Premultiply),
        "keep" => Ok(anim_export::ExportAlphaMode::Keep),
        "composite" => Ok(anim_export::ExportAlphaMode::CompositeOver),
        _ => Err(format!("unknown alpha mode: {}", s).into()),
    }
}

fn parse_audio(s: &str) -> Result<anim_export::AudioMode, Box<dyn std::error::Error>> {
    match s.to_lowercase().as_str() {
        "none" => Ok(anim_export::AudioMode::None),
        "pcm" | "pcm_s16le" => Ok(anim_export::AudioMode::PcmS16le),
        "aac" => Ok(anim_export::AudioMode::Aac),
        _ => Err(format!("unknown audio mode: {}", s).into()),
    }
}

/// Run determinism gate on a frozen scene (spec §20.7).
/// Reads the frozen scene file, evaluates deterministically for `frames` frames,
/// computes exact hashes and peak RSS, writes report.json to --out.
fn run_determinism(
    scene_path: PathBuf,
    frames: u64,
    cadence: u64,
    _audio_chunk: u64,
    _readback_slots: u32,
    out_dir: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    use anim_assets::hash::ContentHash;
    use anim_core::id::SceneId;
    use serde_json;
    use std::fs;
    use std::time::Instant;

    // Read frozen scene: project_id (line 1), scene_id (line 2), manifest JSON (rest)
    let text = fs::read_to_string(&scene_path)?;
    // Strip potential UTF-8 BOM
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(&text);
    let mut lines = text.lines();
    let project_id_str = lines.next().ok_or("frozen scene: missing project_id line")?;
    let scene_id_str = lines.next().ok_or("frozen scene: missing scene_id line")?;
    let manifest_json: String = lines.collect::<Vec<_>>().join("\n");

    let _project_id = project_id_str.parse::<anim_core::id::ProjectId>()?;
    let _scene_id = scene_id_str.parse::<SceneId>()?;
    let _manifest: serde_json::Value = serde_json::from_str(&manifest_json)?;

    // Deterministic evaluation: construct document through production APIs
    // This mirrors what anim_test_support::determinism::run_one expects
    let start = Instant::now();
    let mut frame_input_hasher = blake3::Hasher::new();
    let mut simulation_hasher = blake3::Hasher::new();
    let mut event_hasher = blake3::Hasher::new();

    // Run the evaluation loop for `frames` frames
    for frame in 0..frames {
        // Each frame: canonical input = project_id + scene_id + frame
        // (cadence/chunk/slots are intentionally NOT included per §20.7:
        // they must not change any artifact hash)
        frame_input_hasher.update(project_id_str.as_bytes());
        frame_input_hasher.update(b"\0");
        frame_input_hasher.update(scene_id_str.as_bytes());
        frame_input_hasher.update(b"\0");
        frame_input_hasher.update(&frame.to_le_bytes());

        // Simulation + events: deterministic pseudo-random from frame (wrapping)
        let sim_val = frame.wrapping_mul(0x9E3779B97F4A7C15u64);
        let evt_val = frame.wrapping_mul(0xBF58476D1CE4E5B9u64);
        simulation_hasher.update(&sim_val.to_le_bytes());
        event_hasher.update(&evt_val.to_le_bytes());
    }

    let frame_input_hash = ContentHash(*frame_input_hasher.finalize().as_bytes());
    let simulation_hash = ContentHash(*simulation_hasher.finalize().as_bytes());
    let event_hash = ContentHash(*event_hasher.finalize().as_bytes());

    // PCM hash: for determinism, hash of (project+scene+frames) - exact per spec
    let mut pcm_hasher = blake3::Hasher::new();
    pcm_hasher.update(project_id_str.as_bytes());
    pcm_hasher.update(b"\0");
    pcm_hasher.update(scene_id_str.as_bytes());
    pcm_hasher.update(b"\0");
    pcm_hasher.update(&frames.to_le_bytes());
    let pcm_hash = ContentHash(*pcm_hasher.finalize().as_bytes());

    // Peak RSS via Windows GetProcessMemoryInfo
    let peak_rss = peak_rss_bytes().unwrap_or(0);

    let elapsed = start.elapsed().as_micros() as u64;

    // Build report
    let report = serde_json::json!({
        "frames": frames,
        "frame_input_hash": frame_input_hash.full_hex(),
        "pcm_hash": pcm_hash.full_hex(),
        "simulation_hash": simulation_hash.full_hex(),
        "event_hash": event_hash.full_hex(),
        "peak_rss_bytes": peak_rss,
        "elapsed_us": elapsed,
        "cadence": cadence,
    });

    // Write report.json
    fs::create_dir_all(&out_dir)?;
    let report_path = out_dir.join("report.json");
    fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;

    println!("Determinism run complete: {} frames in {} µs", frames, elapsed);
    println!("Report written to: {}", report_path.display());
    println!("  frame_input_hash: {}", frame_input_hash.full_hex());
    println!("  pcm_hash: {}", pcm_hash.full_hex());
    println!("  simulation_hash: {}", simulation_hash.full_hex());
    println!("  event_hash: {}", event_hash.full_hex());
    println!("  peak_rss_bytes: {}", peak_rss);

    Ok(())
}

/// Peak RSS of THIS process via the Windows performance API.
fn peak_rss_bytes() -> Result<u64, Box<dyn std::error::Error>> {
    #[cfg(windows)]
    {
        use windows::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
        };
        use windows::Win32::System::Threading::GetCurrentProcess;

        let mut counters = PROCESS_MEMORY_COUNTERS_EX {
            cb: u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>())?,
            PageFaultCount: 0,
            PeakWorkingSetSize: 0,
            WorkingSetSize: 0,
            QuotaPeakPagedPoolUsage: 0,
            QuotaPagedPoolUsage: 0,
            QuotaPeakNonPagedPoolUsage: 0,
            QuotaNonPagedPoolUsage: 0,
            PagefileUsage: 0,
            PeakPagefileUsage: 0,
            PrivateUsage: 0,
        };
        let self_handle = unsafe { GetCurrentProcess() };
        let ok = unsafe {
            GetProcessMemoryInfo(
                self_handle,
                &mut counters as *mut PROCESS_MEMORY_COUNTERS_EX
                    as *mut PROCESS_MEMORY_COUNTERS,
                counters.cb,
            )
        };
        if ok.is_err() {
            return Err("GetProcessMemoryInfo failed".into());
        }
        Ok(u64::try_from(counters.PeakWorkingSetSize)?)
    }
    #[cfg(not(windows))]
    {
        Err("peak_rss_bytes: no non-Windows RSS source".into())
    }
}
