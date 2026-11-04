//! WP00 media probe â€” spec 9.5 step 7, 9.7 "Media probe", 9.9 `run_media_probe`.
//!
//! Proves on this machine:
//!   1. explicit-path FFmpeg/FFprobe binaries (from `config/default.toml`) run
//!      and FFprobe returns parsable JSON stream metadata â€” malformed JSON,
//!      missing streams, timeout and nonzero exit are distinct failures
//!   2. WAV / FLAC / OGG(Vorbis) files decode via symphonia with the expected
//!      PCM sample count within declared codec-delay tolerance
//!   3. the LOCKED resampler configuration (rubato sinc, 44.1 kHz â†’ 48 kHz)
//!      produces its declared output count.
//!
//! Children are launched with argument arrays, never a shell (spec 9.4);
//! stderr is drained concurrently and runs have a bounded timeout that
//! terminates and reaps (spec 9.6, 9.11).

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

const SR: u32 = 44_100;
const MUSIC_SECONDS: u32 = 2;
const EXPECTED_PCM: usize = (SR * MUSIC_SECONDS) as usize;
const MAX_CHILD_SECONDS: u64 = 20;

#[derive(Serialize)]
struct MediaProbeReport {
    schema_version: u32,
    capability: String,
    success: bool,
    ffmpeg_version: Option<String>,
    probed: Vec<ProbedSpec>,
    decoded: Vec<DecodedSpec>,
    resampler: Option<ResamplerCheck>,
    diagnostics: Vec<String>,
}

#[derive(Serialize)]
struct ProbedSpec {
    file: String,
    codec: Option<String>,
    sample_rate: Option<u32>,
    duration_secs: Option<f64>,
    streams_declared: usize,
}

#[derive(Serialize)]
struct DecodedSpec {
    file: String,
    decoded_samples: u64,
    expected_samples: u64,
    declared_tolerance: i64,
    exact: bool,
}

#[derive(Serialize)]
struct ResamplerCheck {
    config: String,
    input_samples: usize,
    output_samples: u64,
    expected_output: u64,
    declared_tolerance: u64,
    exact: bool,
}

fn load_tool_path(key: &str) -> Result<PathBuf, String> {
    let raw = std::fs::read_to_string("config/default.toml")
        .map_err(|e| format!("config read: {e}"))?;
    let prefix = format!("{key} = \"");
    for line in raw.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with(&prefix) {
            let rest = &trimmed[prefix.len()..];
            let path = rest.trim_end_matches('"');
            let direct = PathBuf::from(path);
            if direct.exists() {
                return Ok(direct);
            }
            return Err(format!("{key} points to missing binary: {path}"));
        }
    }
    Err(format!("config key `{key}` missing from config/default.toml"))
}

/// Bounded run: argument-array exec, concurrent stdout/stderr drain, watchdog
/// that force-kills after the deadline (terminate and reap, spec 9.11).
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
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| format!("spawn {:?}: {e}", exe.file_name()))?;

    let done = Arc::new(AtomicBool::new(false));
    let watchdog_done = done.clone();
    let pid = child.id();
    let watchdog = std::thread::spawn(move || {
        for _ in 0..MAX_CHILD_SECONDS * 10 {
            if watchdog_done.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if !watchdog_done.load(Ordering::Relaxed) {
            let _ = Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/F", "/T"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    });

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let out_thread = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(mut p) = stdout_pipe {
            let _ = std::io::Read::read_to_string(&mut p, &mut s);
        }
        s
    });
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(mut p) = stderr_pipe {
            let _ = std::io::Read::read_to_string(&mut p, &mut s);
        }
        s
    });
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();
    let status = child.wait().map_err(|e| format!("wait: {e}"))?;
    done.store(true, Ordering::Relaxed);
    let _ = watchdog.join();

    match status.code() {
        Some(code) => Ok((code, stdout, stderr)),
        None => Err(format!("timeout: child (pid {pid}) was force-terminated")),
    }
}

fn spawn_failure(context: &str, msg: &str) -> String {
    format!("[{context}] {msg}")
}

/// Deterministic sine source; returns the float samples (for the resampler)
/// and writes the WAV v;_;_values are exactly known at readback.
fn build_wav(dir: &Path) -> Result<Vec<f32>, String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SR,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(dir.join("src.wav"), spec)
        .map_err(|e| format!("wav create: {e}"))?;
    let mut floats = Vec::with_capacity(EXPECTED_PCM);
    for i in 0..EXPECTED_PCM {
        let t = i as f32 / SR as f32;
        let v = (t * 2.0 * std::f32::consts::PI * 440.0).sin() * 30_000.0;
        floats.push(v);
        writer.write_sample(v as i16).map_err(|e| format!("wav write: {e}"))?;
    }
    writer.finalize().map_err(|e| format!("wav finalize: {e}"))?;
    Ok(floats)
}

fn transcode(
    ffmpeg: &Path,
    dir: &Path,
    input: &str,
    output: &str,
    codec: &[&str],
) -> Result<(), String> {
    let mut args: Vec<&str> = vec!["-y", "-loglevel", "error", "-i", input];
    args.extend_from_slice(codec);
    args.push(output);
    let (code, _out, stderr) =
        run_bounded(ffmpeg, &args, dir).map_err(|e| spawn_failure("ffmpeg", &e))?;
    if code != 0 {
        return Err(format!("ffmpeg exit {code}: {stderr}"));
    }
    Ok(())
}

/// ffprobe â†’ JSON metadata; malformed JSON / missing streams / nonzero exit
/// are distinct failure modes (spec 9.7).
fn ffprobe_json(
    ffprobe: &Path,
    file: &Path,
    dir: &Path,
    report: &mut MediaProbeReport,
) -> Result<(), String> {
    let (code, stdout, stderr) = run_bounded(
        ffprobe,
        &[
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            file.file_name().unwrap().to_str().unwrap(),
        ],
        dir,
    )
    .map_err(|e| spawn_failure("ffprobe", &e))?;
    if code != 0 {
        return Err(format!("ffprobe exit {code}: {stderr}"));
    }
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .map_err(|e| format!("malformed ffprobe JSON: {e}"))?;
    let streams = parsed
        .get("streams")
        .and_then(|v| v.as_array())
        .ok_or("missing streams array in ffprobe JSON")?;
    let first = streams.first().ok_or("no streams present")?;
    let codec = first["codec_name"].as_str().map(String::from);
    let sample_rate = first["sample_rate"].as_str().and_then(|v| v.parse::<u32>().ok());
    let duration = first["duration"].as_str().and_then(|v| v.parse::<f64>().ok());
    report.probed.push(ProbedSpec {
        file: file.file_name().unwrap().to_string_lossy().into_owned(),
        codec,
        sample_rate,
        duration_secs: duration,
        streams_declared: streams.len(),
    });
    Ok(())
}

/// symphonia decode â†’ total interleaved sample count (mono files â†’ frames).
fn decode_count(file: &Path) -> Result<u64, String> {
    let src =
        std::fs::File::open(file).map_err(|e| format!("open {}: {e}", file.display()))?;
    let mss = MediaSourceStream::new(Box::new(src), Default::default());
    let mut hint = Hint::new();
    hint.with_extension(&file.extension().unwrap_or_default().to_string_lossy());
    let mut probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("format probe {}: {e}", file.display()))?;
    let track = probed
        .format
        .tracks()
        .first()
        .cloned()
        .ok_or_else(|| format!("{}: no audio track", file.display()))?;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("decoder init: {e}"))?;
    let mut total: u64 = 0;
    let mut sample_buf: Option<SampleBuffer<i16>> = None;
    loop {
        let packet = match probed.format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(symphonia::core::errors::Error::DecodeError(e)) => {
                return Err(format!("packet error: {e}"))
            }
            Err(e) => return Err(format!("next_packet: {e}")),
        };
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let buf = sample_buf.get_or_insert_with(|| {
                    SampleBuffer::<i16>::new(decoded.capacity() as u64, *decoded.spec())
                });
                buf.copy_interleaved_ref(decoded);
                total += buf.samples().len() as u64;
            }
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue, // skip bad packet
            Err(e) => return Err(format!("decode: {e}")),
        }
    }
    Ok(total)
}

fn resample_check(report: &mut MediaProbeReport, samples: &[f32]) {
    // LOCKED configuration (spec 9.5 step 7): rubato SincFixedIn, quadratic
    // sinc (len 256, BlackmanHarris2 window), chunk 512, 44.1 kHz â†’ 48 kHz.
    // Declared tolerance covers the sinc group delay plus final-chunk
    // rounding: chunk + sinc_len.
    let params = rubato::SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        oversampling_factor: 256,
        interpolation: rubato::SincInterpolationType::Quadratic,
        window: rubato::WindowFunction::BlackmanHarris2,
    };
    let resampler_in =
        rubato::SincFixedIn::<f32>::new(48_000_f64 / SR as f64, 2.0, params, 512, 1);
    let mut resampler = match resampler_in {
        Ok(r) => r,
        Err(e) => {
            report.diagnostics.push(format!("resampler construction: {e}"));
            return;
        }
    };
    // pad to whole chunks so every input sample is fed
    let padded = samples.len().div_ceil(512) * 512;
    let mut input = samples.to_vec();
    input.resize(padded, 0.0);
    let mut output_total: u64 = 0;
    for chunk in input.chunks(512) {
        match rubato::Resampler::process(&mut resampler, &[chunk], None) {
            Ok(out) => output_total += out.iter().map(|ch| ch.len() as u64).sum::<u64>(),
            Err(e) => {
                report.diagnostics.push(format!("resampler error: {e}"));
                return;
            }
        }
    }
    let expected = (EXPECTED_PCM as f64 * 48_000_f64 / SR as f64).round() as u64;
    let tol = 512 + 256;
    let exact = (output_total as i64 - expected as i64).abs() <= tol as i64;
    report.resampler = Some(ResamplerCheck {
        config: "rubato SincFixedIn sinc256 quadratic chunk512 44.1k->48k".into(),
        input_samples: padded,
        output_samples: output_total,
        expected_output: expected,
        declared_tolerance: tol,
        exact,
    });
}

/// Print the FULL detailed report (ProbeReport fields are a subset) and exit
/// with the prescribed status (spec 9.6).
fn emit_detailed(report: &MediaProbeReport) -> ! {
    println!("{}", serde_json::to_string_pretty(report).unwrap_or_default());
    std::process::exit(if report.success { 0 } else { 1 });
}

fn main() {
    let mut report = MediaProbeReport {
        schema_version: 1,
        capability: "media_decode_resample".into(),
        success: false,
        ffmpeg_version: None,
        probed: Vec::new(),
        decoded: Vec::new(),
        resampler: None,
        diagnostics: Vec::new(),
    };

    let ffmpeg = match load_tool_path("ffmpeg_local") {
        Ok(p) => p,
        Err(e) => {
            report.diagnostics.push(e);
            emit_detailed(&report);
        }
    };
    let ffprobe = match load_tool_path("ffprobe_local") {
        Ok(p) => p,
        Err(e) => {
            report.diagnostics.push(e);
            emit_detailed(&report);
        }
    };
    if let Ok((code, out, _)) = run_bounded(&ffmpeg, &["-version"], &PathBuf::from(".")) {
        if code == 0 {
            report.ffmpeg_version = out.lines().next().map(String::from);
        }
    }

    let workdir = std::env::temp_dir().join("wp00_media_probe");
    let _ = std::fs::create_dir_all(&workdir);
    for f in ["src.wav", "v.flac", "v.ogg"] {
        let _ = std::fs::remove_file(workdir.join(f));
    }

    let floats = match build_wav(&workdir) {
        Ok(f) => f,
        Err(e) => {
            report.diagnostics.push(e);
            emit_detailed(&report);
        }
    };
    if let Err(e) = transcode(&ffmpeg, &workdir, "src.wav", "v.flac", &["-c:a", "flac"]) {
        report.diagnostics.push(e);
        emit_detailed(&report);
    }
    if let Err(e) = transcode(&ffmpeg, &workdir, "src.wav", "v.ogg", &["-c:a", "libvorbis"])
    {
        report.diagnostics.push(e);
        emit_detailed(&report);
    }

    // decode verification: declared tolerance per codec-delay behavior (9.7)
    let cases: [(String, u64, i64); 3] = [
        ("src.wav".into(), EXPECTED_PCM as u64, 0), // PCM: no delay
        ("v.flac".into(), EXPECTED_PCM as u64, 0),  // FLAC: exact frame counts
        ("v.ogg".into(), EXPECTED_PCM as u64, 4_096), // Vorbis: encoder delay/pre-skip
    ];
    for (name, expected, tol) in cases {
        match decode_count(&workdir.join(&name)) {
            Ok(n) => {
                let exact = (n as i64 - expected as i64).abs() <= tol;
                report.decoded.push(DecodedSpec {
                    file: name.clone(),
                    decoded_samples: n,
                    expected_samples: expected,
                    declared_tolerance: tol,
                    exact,
                });
                if !exact {
                    report.diagnostics.push(format!(
                        "decoded {name}: {n} vs expected {expected} tol {tol}"
                    ));
                }
            }
            Err(e) => report.diagnostics.push(format!("decode {name}: {e}")),
        }
    }

    // ffprobe metadata for every codec (malformed JSON / missing streams paths)
    for f in ["src.wav", "v.flac", "v.ogg"] {
        if let Err(e) = ffprobe_json(&ffprobe, &workdir.join(f), &workdir, &mut report) {
            report.diagnostics.push(format!("ffprobe {f}: {e}"));
        }
    }

    resample_check(&mut report, &floats);

    let decode_ok = report.decoded.len() == 3 && report.decoded.iter().all(|d| d.exact);
    let probe_ok =
        report.probed.len() == 3 && report.probed.iter().all(|p| p.duration_secs.is_some());
    let resample_ok = report.resampler.as_ref().map(|r| r.exact).unwrap_or(false);
    report.success = decode_ok && probe_ok && resample_ok;
    if report.success {
        report.diagnostics.push("all media checks verified".into());
    }
    emit_detailed(&report);
}
