# Program Status — 2D/2.5D Animation Engine

_Source of truth for "where we are". Updated at the END of every work step._

---

## Snapshot (updated: 2026-11-04, Phase 1.1 done)

| Item | State |
|---|---|
| Current phase | **Phase 1 — Ground & Core Data (1.1 scaffold done; 1.2 next)** |
| Spec document | ✅ Extracted → `docs/spec/des_v3.2_extract.txt` (2,917 paras, 22 WPs) |
| Rust toolchain | ✅ 1.99.1 pinned via `rust-toolchain.toml` (auto-installed on first build) |
| Workspace | ✅ 13-crate workspace compiles; `Cargo.lock` committed authority |
| WP00 gates | ✅ fmt / clippy -D warnings / test --workspace all exit 0 |
| wp00_smoke test | ✅ runs in `crates/anim_test_support/tests/wp00_smoke.rs` |
| Scaffold generator | ✅ `tools/wp00_scaffold.py` fixed (quoted member paths, closing bracket, no clippy.toml) |
| Build-cache redirect | ✅ Confirmed — artifacts in `AppData\Local\anim_engine_target` |

## Next actions (in order)
1. ~~WP00 workspace scaffold~~ ✅ (2026-11-04)
2. ~~WP00 GPU probe~~ ✅ (2026-11-04, PASS — ADR-0002)
3. ~~WP00 adapter probe (LUID correlation + budget)~~ ✅ (2026-11-04, PASS — ADR-0003)
4. WP00 remaining probes: pen (winit HWND + WM_POINTER), media (FFprobe/WAV), export (ffmpeg roundtrip), replace (atomic)
5. Phase 1.2: WP01 rational time (`FramePos`), typed IDs, canonical hashing

## Gate log
| Gate | Result | Date | Evidence |
|---|---|---|---|
| Phase 0.1 — toolchain functional | ✅ PASS | 2026-11-04 | cargo 1.99.1 / rustc 1.99.1; hello-crate compiled + tested in 1.47 s; build cache confirmed outside OneDrive |
| Phase 0.2 — repo initialized | ✅ PASS | 2026-11-04 | first commit contains md files + spec extract + .cargo config |
| Phase 1.1 — WP00 scaffold gates | ✅ PASS | 2026-11-04 | `cargo fmt --check` = 0; `cargo clippy --workspace -- -D warnings` = 0; `cargo test --workspace` = 0 with `workspace_smoke ... ok`; Cargo.lock present |
| WP00 GPU probe | ✅ PASS | 2026-11-04 | `artifacts/probe/wp00_gpu_report.json`: RTX 3050 A via Vulkan (driver NVIDIA 617.14); WGSL compute readback = exact integers; RGBA16F attachment = true; max_buffer 256 MiB @ downlevel limits; exit 0 |
| WP00 adapter probe | ✅ PASS | 2026-11-04 | `artifacts/probe/wp00_adapter_report.json`: both Vulkan devices correlate by 64-bit LUID + vendor/device IDs (Intel luid 69483, RTX luid 123521792); budget query = 3.23 GiB on 4 GB card; negative mismatch test proves no substitution; exit 0. Root cause of initial LUID-invalid: capability extensions + API 1.3 required (ADR-0003) |

## Session journal
| Session | Date | What happened |
|---|---|---|
| Planning | 2026-11-04 | Spec v3.2 received and digested; user confirmed build intent ("are you able" → yes confirmed twice); 5-phase plan approved; memory-file system created (this file, ROADMAP, DECISIONS) |
| Phase 0 | 2026-11-04 | Rust toolchain installed (stable-MSVC, VS Build Tools 18 already present); hello-crate gate PASS; git repo initialized with memory files + spec extract; `.cargo/config.toml` redirects build cache out of OneDrive |
| Phase 1.1 | 2026-11-04 | 13-crate scaffold fixed + verified: members array needed quoted `crates/` paths + closing `]`; smoke test moved into anim_test_support crate; invalid clippy.toml removed (schema rejects lint-name sections) → allow attribute per crate; all three WP00 gates exit 0. Probe/iso debug PNGs cleared from out/arena; QA probes archived to qa2/ |
| Phase 1 GPU probe | 2026-11-04 | `wp00_gpu.rs` written and PASSED on real hardware: wgpu 27.0.1 activated (first build 1m36s cached); Vulkan backend on RTX 3050 A, WGSL dispatch → byte-exact readback, RGBA16F attachment verified; findings recorded in ADR-0001 (workspace/toolchain) + ADR-0002 (backend/adapter rule); machine report in artifacts/probe/wp00_gpu_report.json |
| Phase 1 adapter probe | 2026-11-04 | `wp00_adapter.rs` + shared `probe.rs` PASSED (exit 0): ash 0.38 (same version as wgpu-hal); discovery — NVIDIA/Intel ICDs only fill deviceLUID when external-memory capability exts are active + API 1.3; correlated both Vulkan devices to DXGI by exact 64-bit LUID + vendor/device cross-check; budget = 3.23 GiB of the 4 GB card; negative-path test proves no substitution. ADR-0003 records the load-bearing machine fact |
