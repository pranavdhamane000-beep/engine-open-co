# Program Status — 2D/2.5D Animation Engine

_Source of truth for "where we are". Updated at the END of every work step._

---

## Snapshot (updated: 2026-11-05, end of WP01)

| Item | State |
|---|---|
| Current phase | **WP01 COMPLETE — Phase 1.3 (WP02: immutable assets + content manifest) starts next** |
| WP00 probes | ✅ GPU, ✅ adapter, ✅ media, ✅ export, ✅ replace — 🔒 pen deferred to Phase 4 |
| WP01 core types | ✅ Rational / FrameRate / FramePos / FrameRange / SampleInterval / IDs / Revision / Canonical hashing / atomic write |
| ADRs | 0001 workspace/toolchain · 0002 GPU · 0003 adapter LUID/budget · 0004 media/export/replace · **0005 WP01 encoding decisions** |
| Toolchain | 1.99.1 pinned; `Cargo.lock` frozen (spec §9.5 step 10 satisfied) |

## Next actions (in order)
1. ~~WP00 workspace scaffold~~ ✅ (2026-11-04)
2. ~~WP00 GPU probe~~ ✅ (2026-11-04, PASS — ADR-0002)
3. ~~WP00 adapter probe (LUID correlation + budget)~~ ✅ (2026-11-04, PASS — ADR-0003)
4. **WP00 pen probe DEFERRED to Phase 4 by decision D-10** (user directive)
5. ~~WP00 automated probes: media → export → replace~~ ✅ (2026-11-04, all PASS — ADR-0004)
6. ~~Phase 1.2 (WP01): canonical rational time + stable identity~~ ✅ (2026-11-05, PASS — ADR-0005)
7. **Phase 1.3 (WP02): immutable assets + content manifest** — next checkpointed step
8. Phase 1 closing demo after WP01+ ground crates: save project → hash → reopen → hash identical
9. Reminder: pen probe must complete before Phase 4 editor gates (D-10)

## Gate log
| Gate | Result | Date | Evidence |
|---|---|---|---|
| Phase 0.1 — toolchain functional | ✅ PASS | 2026-11-04 | cargo 1.99.1 / rustc 1.99.1; hello-crate compiled + tested in 1.47 s; build cache confirmed outside OneDrive |
| Phase 0.2 — repo initialized | ✅ PASS | 2026-11-04 | first commit contains md files + spec extract + .cargo config |
| Phase 1.1 — WP00 scaffold gates | ✅ PASS | 2026-11-04 | `cargo fmt --check` = 0; `cargo clippy --workspace -- -D warnings` = 0; `cargo test --workspace` = 0 with `workspace_smoke ... ok`; Cargo.lock present |
| WP00 GPU probe | ✅ PASS | 2026-11-04 | `artifacts/probe/wp00_gpu_report.json`: RTX 3050 A via Vulkan (driver NVIDIA 617.14); WGSL compute readback = exact integers; RGBA16F attachment = true; max_buffer 256 MiB @ downlevel limits; exit 0 |
| WP00 adapter probe | ✅ PASS | 2026-11-04 | `artifacts/probe/wp00_adapter_report.json`: both Vulkan devices correlate by 64-bit LUID + vendor/device IDs (Intel luid 69483, RTX luid 123521792); budget query = 3.23 GiB on 4 GB card; negative mismatch test proves no substitution; exit 0. Root cause of initial LUID-invalid: capability extensions + API 1.3 required (ADR-0003) |
| WP00 media probe | ✅ PASS | 2026-11-04 | `artifacts/probe/wp00_media_report.json`: ffmpeg 9.0.2 explicit-path; ffprobe JSON fine (values are strings!); WAV/FLAC decode exact ±0, Vorbis +440 pre-skip within declared tolerance; locked rubato sinc config verified (ADR-0004) |
| WP00 export probe | ✅ PASS | 2026-11-04 | `artifacts/probe/wp00_export_report.json`: raw RGB → libx264rgb -qp 0 → mkv; byte-identical 16-frame decode; 1 video + 1 audio stream; audio 44,100 exact. KEY FINDING: plain libx264 silently converts RGB→YUV420 (ADR-0004) |
| WP00 replace probe | ✅ PASS | 2026-11-04 | `artifacts/probe/wp00_replace_report.json`: fault-before commit keeps old bytes (blake3), commit swaps complete new, backup holds complete old; same-volume temp files; WP01 implements via ReplaceFileW (ADR-0004) |
| WP00 gate G0 | ✅ RECORD | 2026-11-04 | 5/6 probes PASS; pen deferred by DECISION D-10; toolchain + Cargo.lock frozen |
| WP01 — canonical rational time | ✅ PASS | 2026-11-05 | fmt=0 / clippy -D warnings=0 / test --workspace --locked=0; 32 tests (13 unit + 19 validation). 10,001 adjacent frames at 24000/1001 & 30000/1001 (48k and 44.1k) sample-partitioned with zero drift against BigInt reference; negative exact boundaries floor correctly; NTSC as exact rationals; UUID/Revision/canonical-bytes pinned; ReplaceFileW write-path live in `anim_core::io` (ADR-0005) |

## Session journal
| Session | Date | What happened |
|---|---|---|
| Planning | 2026-11-04 | Spec v3.2 received and digested; user confirmed build intent ("are you able" → yes confirmed twice); 5-phase plan approved; memory-file system created (this file, ROADMAP, DECISIONS) |
| Phase 0 | 2026-11-04 | Rust toolchain installed (stable-MSVC, VS Build Tools 18 already present); hello-crate gate PASS; git repo initialized with memory files + spec extract; `.cargo/config.toml` redirects build cache out of OneDrive |
| Phase 1.1 | 2026-11-04 | 13-crate scaffold fixed + verified: members array needed quoted `crates/` paths + closing `]`; smoke test moved into anim_test_support crate; invalid clippy.toml removed (schema rejects lint-name sections) → allow attribute per crate; all three WP00 gates exit 0. Probe/iso debug PNGs cleared from out/arena; QA probes archived to qa2/ |
| Phase 1 GPU probe | 2026-11-04 | `wp00_gpu.rs` written and PASSED on real hardware: wgpu 27.0.1 activated (first build 1m36s cached); Vulkan backend on RTX 3050 A, WGSL dispatch → byte-exact readback, RGBA16F attachment verified; findings recorded in ADR-0001 (workspace/toolchain) + ADR-0002 (backend/adapter rule); machine report in artifacts/probe/wp00_gpu_report.json |
| Phase 1 adapter probe | 2026-11-04 | `wp00_adapter.rs` + shared `probe.rs` PASSED (exit 0): ash 0.38 (same version as wgpu-hal); discovery — NVIDIA/Intel ICDs only fill deviceLUID when external-memory capability exts are active + API 1.3; correlated both Vulkan devices to DXGI by exact 64-bit LUID + vendor/device cross-check; budget = 3.23 GiB of the 4 GB card; negative-path test proves no substitution. ADR-0003 records the load-bearing machine fact |
| Phase 1 media/export/replace probes | 2026-11-04 | Three automated probes PASSED in one run: (media) ffmpeg at real path D:\ffmp, ffprobe numeric-string lesson, decode tolerances declared, locked rubato config; (export) byte-exact lossless RGB via libx264rgb, stream counts exact, YUV420-conversion trap caught; (replace) ReplaceFileW fault/commit/backup semantics hash-verified. ADR-0004 written; G0 recorded 5/6 + pen deferred (D-10). Config default.toml corrected to real ffmpeg paths |
| Phase 1.2 WP01 | 2026-11-05 | All of `anim_core` implemented per spec §10: error.rs (7 variants incl. message-only Io), rational.rs (i128 + BigInt-reduced arithmetic via `from_bigints`, floor/ceil checked), time.rs (FrameRate with call-site validation for struct-literal bypass, FramePos carry-normalize + strict canonical deserialize, frame_samples BigInt exact, sample_at), id.rs (7 typed UUID wrappers, bare-UUID Display/FromStr round trip, Revision checked), hash.rs (length-framed domain blake3 + Canonical for primitives/Rational/ids/ranges), io/ (atomic write: temp→sync→ReplaceFileW→reopen-verify). Validation: 10,001-frame partition at both NTSC rates + 44.1k zero-drift vs BigInt reference; 5 proptests × 512 cases; 32 tests total; gates fmt/clippy/test all 0. ADR-0005 records encoding decisions; proptest macro quirk learned (needs `#[test]` attr inside; no path invocation) |
