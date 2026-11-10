# ADR-0013 — WP09 Headless Export, FFmpeg and Verified Publication

**Status**: Accepted
**Date**: 2026-11-09
**Work package**: WP09 (spec §18, extract lines 956–1048)
**Supersedes**: None
**Related**: ADR-0005 (rational time), ADR-0006 (assets/hashing), ADR-0010 (render graph), ADR-0012 (offline audio)

## Problem

WP08 delivered deterministic offline audio. WP09 adds the complete headless export path: validating settings, rendering frames with bounded GPU readback, encoding through external FFmpeg, verifying output, and publishing atomically. Errors must abort before publication, never retry at lower quality, and never fall back to unverified codecs.

## Decisions

### D1 — FFmpeg spawned directly, no shell (spec §18.3)
Processes are spawned via `tokio::process::Command` from `FFmpegTools` explicit paths (resolved with `which`, version + BLAKE3 hash captured). No shell, no cmd wrapper, no argument re-quoting.

### D2 — FFmpegTools as the single environment record (spec §18.3, §18.7)
`FFmpegTools { ffmpeg_path, ffprobe_path, ffmpeg_version, ffprobe_version, ffmpeg_hash: String }` is built once at resolution; the export manifest records the version strings and hash so exports are reproducible and verified against the actual binary.

### D3 — Settings validation gates everything (spec §18.5 step 1)
`ExportSettings::validate` runs before any work: schema, frame ordering, dimensions, even-dimension rule for codecs that require it, alpha-vs-profile compatibility. `ResolvedExport` binds validated settings to the codec profile and tool environment.

### D4 — Locked codec profiles (spec §18.6)
Codec × quality × alpha resolve to fixed argument sets: H.264 MP4/MOV (`libx264`, CRF by quality tier, yuv420p, `+faststart`), ProRes MOV (yuv444p10le, alpha-capable), PNG sequence. Profiles declare `supports_alpha` and `requires_even_dimensions`. No unverified fallback.

### D5 — Readback ring with backpressure (spec §18.7, §18.8)
`ReadbackRing` holds a fixed-capacity set of GPU→CPU staging buffers. Slots move Free → Submitted → Mapped → Ready → Free. Rendering cannot run more than slot capacity ahead of FFmpeg; rows are unpadded (256-byte alignment removed) before writing raw frames. `RingStats` exposes capacity/submitted/completed/in-flight for diagnostics.

### D6 — Job manifest (spec §18.3, §18.6)
`ExportManifest` records project/settings/asset hashes (BLAKE3 hex), FFmpeg version+hash, resolved profile name, timestamps, working directory, and destination. The manifest is written with the export and is the reproducibility record.

### D7 — Cancellation and progress as separate contract (spec §18.9)
`CancellationToken` (atomic flag, `check() → ExportResult<()>`) and `ProgressSink` (`on_stage`, `on_progress`, `on_error`) are re-exported from `progress.rs`. `ConsoleProgressSink` implements console reporting. Pipeline stages advance through the `PipelineStage` enum; sinks cannot alter timing or quality.

### D8 — Atomic publication, same volume (spec §18.7)
Isolated job temp paths are created beside the final destination (`create_job_temp_dir`). Single files publish via rename on the same volume (`publish_file_atomic`); PNG sequences publish via complete-sibling-directory rename (`publish_png_sequence`). Never delete-then-rename. A previous destination is untouched on verification or publication failure.

### D9 — Verification before publication (spec §18.9)
`verify_output` runs FFprobe (JSON) checking codec, dimensions, pixel format, rational frame rate, and audio stream properties (48 kHz stereo when audio expected). `verify_video_decoded` decodes the final file to count exact frames. Verification failure blocks publication.

### D10 — Export pipeline as staged state machine (spec §18.5)
`ExportPipeline` walks Validating → PreparingSimulation → RenderingAudio → StartingEncoder → RenderingFrames → EncodingFrames → FinalizingEncoder → Muxing → VerifyingOutput → Publishing → Completed (Failed on error). Each stage reports through the sink and checks cancellation.

### D11 — Errors never degrade quality (spec §18.11)
`ExportError` covers validation, asset, simulation, GPU, memory, encoder, mux, verification, io, cancellation, publication. Any error fails the export; there is no retry at lower quality and no codec substitution. Broken pipe includes FFmpeg status; timeouts kill and reap the child.

### D12 — Export CLI surface (spec §18.5)
`anim_headless` composes `budget_report` (WP07 diagnostics) and `export` binaries, plus a validated `clap` CLI schema parsing frames/fps/codec/quality/alpha/audio with strict error mapping. Unknown values are rejected, not guessed.

## Consequences

- anim_export compiles as 9 modules (error, settings, manifest, ffmpeg, readback, pipeline, progress, publish, verify) + lib; anim_headless gains cli.rs + export.rs + the closure of the WP09 compile-fix loop.
- All gates pass: fmt=0, clippy --workspace --all-targets=0, test --workspace --locked=0 (46 suites, 250+ tests).
- Deferred to real-export smoke (G-gate): full end-to-end FFmpeg encode against a real RTX 3050 render pass is validated in WP11's 10,000-frame determinism gate; WP09 scope here is API, argument construction, and process machinery.

## G7 Validation

- Unit: settings validation (schema, frame ordering, zero dims, odd dims, alpha-vs-profile), manifest round-trip (write/read), argument builders (rawvideo header, fps triple, audio handoff, codec args, output), readback invariants, cancellation flag semantics.
- Live machine regression: `budget_report` still green on RTX 3050 (WP07 continuity).
