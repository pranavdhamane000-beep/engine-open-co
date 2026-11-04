# ADR-0004 — Media, export, and atomic-replacement platform facts

**Status:** Accepted · **Date:** 2026-11-04 ·
**Evidence:** `artifacts/probe/wp00_media_report.json`, `wp00_export_report.json`, `wp00_replace_report.json`

## Context

Spec §9.5 steps 7–9 require proving FFprobe metadata + decode/resample, the
video/audio export roundtrip, and Windows atomic replacement before WP01+.
The pen probe is deferred to Phase 4 by decision D-10.

## Decision / verified facts

### Media (media probe, exit 0)
1. **Tool paths:** FFmpeg lives at `D:\ffmp\ffmpeg-9.0.2-essentials_build\bin\`
   (scaffold had assumed `C:\ffmpeg` — corrected in `config/default.toml` and
   the scaffold generator; "no PATH discovery" rule satisfied by explicit
   absolute paths).
2. **ffprobe JSON emits numbers as strings** (`sample_rate`, `duration`,
   `nb_read_frames`) — all consumers parse-then-use; noted for WP09.
3. **Decode counts:** WAV and FLAC decode **exactly** (88,200 samples, ±0);
   Vorbis decodes with +440 encoder pre-skip (within declared 4,096
   tolerance). WP08 audio planning must not treat container length as exact —
   use sample counts from decode, with per-codec delay declarations.
4. **Locked resampler config:** rubato `SincFixedIn`, sinc 256 quadratic
   (BlackmanHarris2), chunk 512, 44.1 k → 48 k; output count matches
   expectation within declares sinc group delay + chunk rounding. This exact
   configuration is the WP08 baseline.

### Export (export probe, exit 0)
1. **RGB color must use `libx264rgb -qp 0`.** Plain `libx264` silently
   converts RGB→YUV 4:2:0 (decoded bytes differed even at -qp 0) — caught by
   byte-exact verification. WP09's render-graph writeout (linear-specialized,
   spec §17) must therefore EITHER ask the graph for YUV 4:2:0 output AND
   document the transform, OR use an RGB-capable lossless codec. Never
   "verify" color correctness by eyeballing a compressed preview.
2. **Stream counting:** `ffprobe -count_frames` + JSON parse confirmed
   exactly 1 video + 1 audio stream, 16 frames, 44,100 audio samples exact.
3. **Matroska** is the container with lossless-PCM support in the essentials
   build; MP4/PCM (uncompressed) is NOT supported by that muxer.

### Replacement (replace probe, exit 0)
1. `ReplaceFileW` with a backup atomically swaps destinations; the backup
   receives the complete OLD state; fault-before-commit (missing source)
   leaves the destination untouched (blake3-verified old hash) — crash-safe
   save semantics proven on this filesystem.
2. The production implementation lands at WP01 (`anim_core/src/io/*`) using
   exactly this API + flush-before (write→sync→replace), same-volume paths.

## Consequences

- WP00 is complete except the deferred pen probe (D-10).
- G0 gate recording: 5/6 probes PASS by evidence artifacts; 1 deferred by
  written decision; toolchain + `Cargo.lock` frozen (§9.5 step 10).
- WP01 may start from these verified primitives.
