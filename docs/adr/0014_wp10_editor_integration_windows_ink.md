# ADR-0014 — WP10 Editor Integration and Windows Ink

**Status**: Accepted
**Date**: 2026-11-09
**Work package**: WP10 (spec §19, extract lines 1049–1160)
**Supersedes**: None
**Related**: ADR-0007 (WP03 command boundary), ADR-0008 (WP04 snapshots), ADR-0009 (WP05 simulation), ADR-0010 (WP06 render graph, goldens), ADR-0012 (WP08 preview clock), ADR-0013 (WP09 export)

## Problem

WP09 closed the headless export path. WP10 adds the interactive editor layer: session/transport/preview-quality/ink inside `anim_editor` (pure, testable) and the winit application composition in `anim_app`. The editor must mutate ONLY through the WP03 single-writer command boundary, read only immutable snapshots, run adaptive preview-quality confined to interactive preview, and support Windows Ink strokes that preview transiently and commit EXACTLY ONCE.

## Decisions

### D1 — Session as single-writer adapter (spec §19.6, §19.9)
`EditorSession` wraps the WP03 `CommandProcessor` behind a `Mutex<Inner>`. Every mutation flows through `CommandEnvelope`; every read publishes an immutable `Arc<EditorSnapshot>` (`CommandSink` + `SnapshotReader` traits in `lib.rs`). UI code never holds mutable scene references; panels and viewport change state only by issuing commands.

### D2 — Editor transport stores plain i64 (deviation, editor-local)
`EditorTransport` keeps `position_frame: i64` (NOT the `FramePos` newtype). WP01 rationals remain the canonical time at scene/sim/export boundaries; the editor transport is transient UI state, and avoiding the newtype keeps the audio-clock ingestion verbatim (`u64` generations, pl `PreviewClockSnapshot` fields used exactly).

### D3 — Frame-step paused-only, saturating (spec §19.5)
`step_frames` returns a typed `EditorError::Transport` when playing (check BEFORE any mutation) and saturates arithmetic so runaway steps cannot panic the event loop (§19.11).

### D4 — Hysteretic preview quality, preview-ONLY (spec §19.7, §19.8)
`PreviewQualityController`: EMA α = 0.125 over measured frame times; downgrade ONE level after 8 consecutive over-budget frames; upgrade ONE level only after 120 consecutive frames under 70% of target; scales exactly 1.0 / 0.75 / 0.5 (Half = floor, Full = ceiling); single-level steps prevent skip transitions. `reset_counters()` on breakpoint/seek/suspension/resize; `reset_full()` on device recovery and session start. State is session-local and NEVER serialized — `request_export` takes caller-supplied `ExportSettings` and pre-validates them, so quality state cannot leak into project or export settings (test-enforced).

### D5 — EMA decay lag shapes upgrade validation (test-method decision)
Because the EMA must decay before the consecutive-under streak can begin after a burst, a naive "120 raw frames" test fails empirically. Upgrade behavior is therefore validated SCAN-BASED: feed a steady under-budget stream, record the measured upgrade index, and assert `index ≥ 120` — the hysteresis floor holds without asserting a false exact frame count.

### D6 — Ink state machine with frozen setup (spec §19.6)
`InkEngine` states: Idle → Capturing → CommitPending → Committed → Idle; CaptureLost/FocusLost/Cancel → Cancelled. `StrokeSetup` (view transform, scene/node target, key time) is frozen AT POINTER-DOWN and cloned into the engine by the session; updates use the frozen copy. Coalesced history must be ordered oldest→newest (enforced: out-of-order history is a typed `InvalidPointerData` rejection of the whole event).

### D7 — Exactly-once stroke commit under a stable command id (spec §19.7)
Pointer-up freezes `CommitSpec { stroke_id, command_id, setup, points }`; the session builds ONE envelope: `SetNodeProperty { property: "ink.stroke", key: PropertyKey { time: Rational(num,1), value: Text(StrokePayloadV1 JSON), easing: Step } }` carrying the STROKE's command id (overriding `CommandEnvelope::new`'s fresh id) so retries are recognized as the same command (WP03 idempotency). `CommandError::Persistence` → `EditorError::RetryableCommit` — stroke stays pending with the SAME id; any other error → `CommandRejected` with explicit cancel available; a durable outcome → `mark_committed` exactly once, overlay dropped, rendering reads snapshots only (no double rendering).

### D8 — Malformed input is typed, never a crash (spec §19.11)
Nonfinite coordinates/pressure → `InvalidPointerData` with state preserved. Duplicate up / synthesized mouse / non-primary / wrong pointer id are ignored (mouse editing preserved when ink is disabled — `Tool::Disabled` passes everything through as `Ignored`). Eraser input selects an explicit eraser command path, never a drawing stroke. `MAX_STROKE_POINTS = 4096`: exceeding cancels the stroke with a visible diagnostic (`MaxPointsExceeded`), committing NOTHING.

### D9 — Win32 pen conversion against the real projection (spec §19.3, D-10 scope)
`anim_app/src/win32_ink.rs` (cfg-windows) converts `POINTER_PEN_INFO` frames to editor `PointerEvent`s: pressure 0..=1024 → [0,1]; dwTime ms → µs ×1000; coordinates from `ptPixelLocation`; tilt i32 → i16. The windows-0.62 projection exposes PPI FLAT (`penFlags: u32`, `pressure: u32`, `tiltX/Y: i32` — no nested `penInfo`) and OMITS the PEN_FLAG_* constants, so the documented Win32 values are carried locally (ERASER=0x1, INVERT=0x2, BARREL=0x4). `GetPointerType(id, *mut POINTER_INPUT_TYPE) -> Result` (pointer_id, not u32 return; POINTER_INPUT_TYPE is i32-backed) filters pen pointers. Native WM_POINTER window subclassing is deferred to WP11 app integration per spec (degraded path: a subclass failure disables Ink with an explicit diagnostic while mouse editing survives).

### D10 — winit composition, no GPU selection (D-11, spec §19.10)
`anim_app/src/main.rs`: winit `ApplicationHandler` owns the event loop; `window.rs` is a pure `AppEvent → Action` router (testable without GPU); redraws are always explicit; resize/focus route without hidden edit paths; space key → transport play via the session API. The D-11 startup reminder (display-only adapter listing, "make sure the dGPU renders") is preserved verbatim. Orderly shutdown: pause transport, publish, THEN the window closes.

### D11 — Pen probe discipline becomes a pre-gate step (environment lesson)
The RTX 3050 dropped off the system mid-session (only IntegratedGpu adapters + Intel in Win32_VideoController) and the WP06 goldens failed 2/8 BY DESIGN on the Intel fallback (D-11 forbids auto-swapping adapters; goldens are dGPU-calibrated). A `wp00_gpu` adapter-presence probe is now part of the pre-gate checklist BEFORE running GPU-dependent suites. Related: `link.exe` exit 1318/1140 mid-gate was disk exhaustion (C: at 0 bytes free, os error 112 surfaced on a fingerprint write) — cleared the build-cache `incremental` dir (3.4 GB); disk headroom is a standing precondition for the build.

## Consequences

- anim_editor: 5 modules (error, ink, preview_quality, session, transport) + lib; 24 tests green (8 lib + 10 ink + 6 preview_quality).
- anim_app: window router (2 tests) + win32_ink conversion layer + composed main; 7/7 bin tests.
- All three gates green on RTX 3050: fmt=0, clippy --workspace --all-targets=0 errors, test --workspace --locked=0 (goldens 8/8 restored once the dGPU returned).
- Remaining WP10 surface (panels/viewport/diagnostics per spec 19.3–19.5) lands with WP11 app integration, where egui docking and the native pen subclass wire into the session — the pure-logic contracts defined here are the integration target.
- Detection improvement adopted: adapter probe (D11) before any GPU-dependent gate run.

## G-WP10 Validation

- Ink: exactly-once commit + duplicate-up no-op; mouse-synthesis/non-primary/wrong-pointer never inject; capture-loss cancel without edit; eraser path; malformed-event typed rejection with preserved state; history-order enforcement; pressure clamp; 4096-point cancel; same-id retry retention; session-level single-undo-entry commit (setup + stroke = 2); disabled-tool passthrough.
- Preview quality: 8-over downgrade; scan-based ≥120-under upgrade; half floor (holds under continued load); seek resets counters; export-settings hash invariance under any quality churn; recovered session starts at best.
- Transport: paused-only stepping (check before mutation); saturating steps; explicit seek; verbatim audio-clock generations.
- Machine-gate rerun: full `test --workspace --locked` PASS after dGPU revival.
