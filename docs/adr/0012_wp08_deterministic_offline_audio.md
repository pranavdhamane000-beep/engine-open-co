# ADR-0012 — WP08 Deterministic Offline Audio and Preview Clock

**Status**: Accepted
**Date**: 2026-11-08
**Work package**: WP08 (spec §17, extract lines 890–954)
**Supersedes**: None
**Related**: ADR-0005 (WP01 rational time), ADR-0006 (WP02 assets), ADR-0008 (WP04 scene), ADR-0009 (WP05 simulation), ADR-0011 (WP07 memory)

## Problem

WP07 completed the render pipeline with budget-aware GPU memory management. WP08 adds deterministic offline audio rendering and an independent real-time preview clock, per spec §17. The offline path must produce bit-exact stereo 48 kHz PCM for export. The preview path provides real-time audio driven by the audio output stream, isolated from the offline path so that device state, callback timing, or preview transport never alter offline output.

## Decisions

### D1 — Exact rational frame→sample conversion (spec §17.7, §17.8)
Frame-range conversion uses WP01 exact rational arithmetic: `SampleInterval = frame_samples(frame, 48000)` at both boundaries. The `FrameRate::frame_samples` function uses BigInt arithmetic (`floor(f * 48000 * den / num)`) with checked conversion to i64. No floating-point seconds ever determine clip placement.

### D2 — Symphonia for decoding (spec §17.3)
Source audio (WAV, FLAC, OGG, MP3) decoded via symphonia 0.5 with enabled format features. Decoded PCM is cached by `(content_hash, decoder_version, resampler_version, settings_hash)`. Streaming resampler state is private to each asset decode session and independent of output mix chunk size.

### D3 — Locked rubato SincFixedIn configuration (spec §17.3, §17.7)
Resampler parameters are FIXED for determinism:
- `sinc_len: 256`
- `f_cutoff: 0.95`
- `interpolation: Linear`
- `oversampling_factor: 256`
- `window: BlackmanHarris2`
These values are compile-time constants (`RESAMPLER_PARAMS`). The resampler processes each channel independently via `SincFixedIn<f64>`.

### D4 — Exact 48 kHz stereo canonical format (spec §17.8)
All offline audio is resampled to 48,000 Hz, 2 channels (stereo), i16. Mono sources are duplicated to both channels. Output sample rate and channel count are compile-time constants (`OUTPUT_SAMPLE_RATE = 48_000`, `OUTPUT_CHANNELS = 2`).

### D5 — Ordered mixing with f64 accumulator (spec §17.7)
Clips are processed in canonical order (timeline start, then clip ID). For each output sample, all intersecting clips contribute. Accumulation uses f64 for stable summation. Mono clips are gained and mixed to both channels. Stereo clips preserve channel separation. No random dither. No adaptive normalization.

### D6 — Equal-power fade curves (spec §17.7)
Fade-in/out uses equal-power cosine curve: `sin(t * π/2)` for fade-in, `cos(t * π/2)` for fade-out, where `t = pos / len`. Linear fade is available as an alternative. Fade lengths are validated against source length.

### D7 — Symmetric i16 rounding (spec §17.7)
After clamping to [-1, 1], values are rounded once to i16 using symmetric rounding: positive values add 0.5 then truncate; negative values subtract 0.5 then truncate. This avoids DC bias. No random dither.

### D8 — Offline rendering as pure function (spec §17.6, §17.9)
`OfflineRenderer::render` is a pure function: `(SceneSnapshot, AssetResolver, OfflineAudioSpec, Writer) → AudioResult<u64>`. Same inputs always produce identical WAV bytes. Timeline scheduling uses integer samples; floating-point seconds never determine clip placement. Resampler implementation, coefficients, channel conversion, mix order, and output rounding are versioned. No random dither.

### D9 — Preview transport isolation (spec §17.3, §17.7, §17.8)
Preview owns a separate `PreviewTransport` with its own clock state (generation, anchor, device samples, rate, playing flag, discontinuity flag). The audio callback consumes preallocated PCM blocks, increments the device-sample counter, and emits silence on underrun. It does NOT allocate, block on disk/GPU work, or acquire the project lock. Pause, seek, rate change, underrun, and device restart all publish clock-generation changes so video does not infer continuity across a discontinuity.

### D10 — cpal stream with lock-free ring buffer (spec §17.3, §17.9)
Real-time preview uses cpal 0.16 with a preallocated `HeapRb<i16>` ring buffer (lock-free, `preview-audio` feature only). The callback fills from the ring buffer first; on underrun it calls the transport callback for a fresh mix block, pushes to the ring, then continues. Format is enforced: 48 kHz, stereo, i16.

### D11 — Canonical little-endian WAV output (spec §17.3, §17.9)
`write_wav` writes a valid RIFF/WAVE with fmt chunk (PCM, 48 kHz, stereo, 16-bit) and data chunk. Data is written in chunks of `chunk_samples` frames (default 1024), proving chunk-size invariance. `write_wav` returns the number of sample frames (not interleaved scalars).

### D12 — Typed errors and public API (spec §17.11, §17.12)
All errors are typed (`AudioError`/`PreviewError` with `thiserror`). Offline errors abort before publication. Preview device errors stop audio, increment generation, and leave editing available. No error silently shifts clip offsets or replaces a source with silence. The public API exports: `OfflineAudioSpec`, `PlannedClip`, `SampleOffset`, `SampleRange`, `compile_plan`, `MixPlan`, `decode_asset`, `DecodedAsset`, `DecodeCacheKey`, `resample_to_canonical`, `StreamingResampler`, `OUTPUT_SAMPLE_RATE`, `OUTPUT_CHANNELS`, `mix_range`, `PcmAssetSource`, `MemoryAssetSource`, `FadeCurve`, `OfflineRenderer`, `render_offline`, `PreviewTransport`, `PreviewClockSnapshot`, `TransportState`, `write_wav`, `write_wav_streaming`, and under `preview-audio`: `PreviewOutput`.

## Consequences

- The audio crate compiles clean with 12 modules + 2 test files.
- All gates pass: `fmt=0`, `clippy --workspace --all-targets=0 warnings`, `test --workspace --locked=0` (45 test suites, 250+ tests).
- Offline rendering is deterministic: same inputs → bit-exact WAV output.
- Preview transport is isolated from offline rendering; preview device errors do not affect offline output.
- Chunk-size invariance proven: varying `chunk_samples` (512, 1024, 2048) produces identical WAV output.
- Frame→sample conversion exact: 100 frames at 24000/1001 fps = 200200 samples exactly.
- Preview transport clock snapshots track generation, device samples, anchor timeline sample, rate, playing state, and discontinuity flag.

## Alternatives considered

- **cpal for offline**: Rejected — offline must never call cpal or wall-clock APIs (spec §17.4).
- **Variable resampler settings**: Rejected — coefficients/settings must be versioned and locked (spec §17.6).
- **f32 accumulation**: Rejected — f64 provides stable summation for long mixes (spec §17.7).
- **Adaptive normalization**: Rejected — would break determinism (spec §17.7).
- **Shared preview/offline state**: Rejected — preview must not alter offline output (spec §17.6).

## Evidence

- All workspace gates pass: `fmt=0`, `clippy --workspace --all-targets=0 warnings`, `test --workspace --locked=0` (45 test suites, 250+ tests).
- `anim_audio` tests: 4 offline + 7 preview = 11 tests pass.
- Full workspace test run: 45 test suites pass (250+ individual tests).
- Golden reference: frame 100 at 24000/1001 fps = exactly 200200 samples at 48 kHz.

## ADR numbering note

This is ADR-0012 following sequential numbering (ADR-0011 = WP07 DXGI budget memory management).