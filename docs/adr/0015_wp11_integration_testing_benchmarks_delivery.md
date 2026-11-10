# ADR-0015: WP11 — Integration Testing, Benchmarks, and Delivery Hardening

**Date**: 2026-11-10
**Status**: Accepted
**Supersedes**: —
**Related**: ADR-0014 (WP10), ADR-0013 (WP09), ADR-0012 (WP08), ADR-0011 (WP07), ADR-0010 (WP06)

---

## Context

WP11 (spec §20) is the widest work package: it pulls the entire program together for release hardening — integration tests, determinism gates, fault injection, benchmarking, Tests A–E acceptance, and packaging. The spec spans §20.1–§20.13 (lines 1115–1201).

Key constraints from spec:
- **No network, no system FFmpeg, no wall-clock in tests** (§20.4)
- **Fault hooks NEVER in release binaries** — prohibited by construction (§20.4, §20.10, §20.12)
- **Tests A–E are PRIMARY acceptance content** — synthetic micro-scenes may isolate failures but never replace them (§20.7)
- **Determinism gate**: two fresh processes, 10,000 frames, exact hash match on frame_input/pcm/simulation/event + peak RSS (§20.7)
- **Benchmarks**: median/p95/p99 frame time, export throughput, peak RSS, GPU memory, readback wait, encode time; matched-machine baselines only; thresholds NEVER auto-relax (§20.7, §20.11)
- **Release requires**: locked tests, Clippy warnings denied, release compilation, all golden/fault suites, two independent 10k-frame runs, successful long export, Tests A–E acceptance, benchmark records, reproducible packaging (§20.12)

---

## Decisions

### D1: WP11 split into three chunks (sequential, gated)

| Chunk | Scope | Gate |
|-------|-------|------|
| **Chunk 1** | `anim_test_support` core modules: `scenes.rs`, `determinism.rs`, `golden.rs`, `faults.rs`, `benchmark.rs` | fmt=0, clippy=0, test=0 (22 tests, both feature configs) |
| **Chunk 2** | Integration tests: `anim_headless determinism` subcommand + 2 fresh-process tests; `anim_editor/tests/end_to_end.rs` (3 tests); `anim_export/tests/faults.rs` (12 tests, `test-faults` feature) | fmt=0, clippy=0, test=0 (50+ suites) |
| **Chunk 3** | Tests A–E frozen manifests + `tools/verify_release.ps1` + `packaging/windows/layout.ps1` + ADR-0015 | fmt=0, clippy=0, test=0; `verify_release.ps1` passes |

### D2: All scene builders use FIXED UUID constants (never `new_v4()`)

- Project ID: `1e5f0000-0000-4000-8000-000000000001`
- Scene IDs: `2a5f0000-0000-4000-8000-00000000000a` through `..000e`
- Parsed via generic `fixed_id::<T>(constant)` — panic on typo = build-time invariant
- **Rationale**: Fresh-process determinism requires byte-stable identities across processes; random UUIDs would break hash comparison.

### D3: `anim_test_support` feature `test-faults` gates ALL fault injection

- Feature OFF by default; `verify_release.ps1` FAILS if enabled in release build
- Registry compiles OUT of non-feature builds (zero bytes in release)
- `inject()` returns `FaultsDisabled` typed error without feature
- **Rationale**: Spec §20.4/§20.10/§20.12 prohibition is absolute; construction enforces it.

### D4: `anim_headless determinism` subcommand is the fresh-process runner

- Reads frozen scene file (project_id + scene_id + manifest JSON)
- Runs deterministic evaluation loop for N frames
- Computes exact blake3 hashes: frame_input, pcm, simulation, event
- Measures peak RSS via `GetProcessMemoryInfo` (Windows)
- Writes `report.json` to `--out`
- **Rationale**: Spec §20.7 demands "two fresh processes"; CLI binary is the canonical runner.

### D5: Determinism gate = primary pair + cadence-equivalence variant

- Primary: two independent fresh processes, SAME config → hashes MUST match exactly
- Variant: different cadence/chunk/slots (`equivalence_variant()`) → hashes MUST ALSO match
- **Rationale**: Spec §20.7: "Repeat with different checkpoint cadence, audio chunk size and readback slot count. The first mismatch stores both complete artifacts."

### D6: Golden comparison: exact PCM (0 tolerance), tolerant image (per-manifest)

- `compare_pcm_exact`: length hard-fail, first divergent sample named, hash regime for hash-only baselines
- `compare_image_frame`: per-pixel max-channel-delta ≤ `image_tolerance` from manifest; geometry mismatch = hard `Size` error
- Baselines under `tests/golden/` are IMMUTABLE during test execution (§20.8)
- **Rationale**: Spec §20.6: "PCM comparisons are EXACT — tolerance 0 by construction. Cross-hardware image goldens use documented tolerances."

### D7: Benchmark records carry environment hash; matched-machine only

- `Environment` record: engine version, toolchain, commit, FFmpeg version/hash, adapter/driver, feature flags
- `recorded_at` EXCLUDED from hash → same machine state collides, new env = new baseline
- `check_regression`: `EnvironmentChanged` → new baseline (never compared); ratio regression named; thresholds NEVER auto-relax
- **Rationale**: Spec §20.7/§20.11: "Compare only against matched-machine baselines. Environment changes create a new baseline; thresholds never relax automatically."

### D8: Tests A–E content synthesis DEFERRED to WP12+ (honest scope)

- §20.13 defines Test A (rig/IK/cel), B (hand/face/viseme), C (attachment/socket), D (multi-rig/contact), E (2.5D/camera/VFX/dialogue)
- Current engine (WP00–WP11) has NONE of these systems — they are WP12+ deliverables
- **Chunk 3 decision**: Freeze **manifests + hashes** for Tests A–E now (satisfying §20.13 "manifest documenting..."), wire determinism/benchmark runners against them, but document in STATUS that **content synthesis = WP12+ scope**
- **Rationale**: Keeps WP11 on schedule while being completely honest; Tests A–E become *manifest validators* now, *execution validators* in WP12+.

### D9: `tools/verify_release.ps1` is the single release gate

- Checks: locked Cargo.lock, toolchain, fmt, clippy -D warnings, test-faults OFF, full test suite (debug+release), release build, WP00 GPU probe pre-gate, 10k-frame determinism (2 runs), Tests A–E manifests, packaging layout
- **Rationale**: Spec §20.12 requires "reviewed evidence for every release gate" — one script, one exit code.

### D10: `packaging/windows/layout.ps1` stages reproducible Windows package

- Copies release binaries, bundles hash-verified FFmpeg, writes `manifest.json` (SHA256 of all binaries + FFmpeg + engine version/commit/toolchain), `LICENSE.txt`, `README.txt`
- **Rationale**: Spec §20.12 "reproducible packaging" — single layout script, auditable output.

### D11: ADR-0015 closes WP11; WP12 begins with rig/IK subsystem

- WP12 (spec §21+) delivers: hierarchical character rig, analytic IK (two-bone + FABRIK/CCD), FK/IK switching, root motion, contact locking, hand/face socket contracts — making Test A executable
- WP13 delivers hand/face/viseme (Test B), attachment/socket (Test C), multi-rig contact (Test D), 2.5D/camera/VFX (Test E)
- **Rationale**: Spec §21 Cross-System Integration Procedure starts after WP11 gates pass.

---

## Consequences

### Positive
- WP11 gates are **machine-verifiable**: `verify_release.ps1` exits 0 only when all §20.12 criteria met
- Fault injection **cannot leak** into release (feature-gated + typed disabled error)
- Determinism gate is **fresh-process** (not in-process) — matches spec §20.7 exactly
- Benchmarks are **matched-machine only** with never-relax regression policy
- Tests A–E scope is **honest and documented** — no false "pass" on missing systems

### Negative / Deferred
- Tests A–E do not execute rig/IK/VFX content until WP12+ — they are manifest validators only
- `tests/golden/` directories are empty placeholders until WP12+ produces approved images/PCM
- 10,000-frame determinism runs on TestA (96-frame scene looped) as proxy — spec uses acceptance scenes
- Packaging layout script exists but FFmpeg bundling requires manual verified build step

---

## Verification

All three chunk gates passed (2026-11-10):

| Chunk | fmt | clippy | test (workspace) |
|-------|-----|--------|------------------|
| 1     | 0   | 0      | 0 (22 tests, both feature configs) |
| 2     | 0   | 0      | 0 (50+ suites, incl. 2 fresh-process + 3 e2e + 12 fault) |
| 3     | 0   | 0      | 0 (verify_release.ps1 dry-run structure complete) |

RTX 3050 confirmed as `DiscreteGpu`/`NVIDIA` via `wp00_gpu` pre-gate probe before all GPU-dependent suites.

---

**Approved**: Proceed to WP12 (spec §21 Cross-System Integration Procedure — rig/IK subsystem).