# ADR-0002 — GPU backend and adapter selection rule

**Status:** Accepted · **Date:** 2026-11-04 · **Evidence:** `artifacts/probe/wp00_gpu_report.json`

## Context

Spec §9.5 step 4–5 and §9.7 require proving adapter enumeration, WGSL compute
dispatch, and buffer readback on the reference machine (RTX 3050 A laptop)
before any render architecture is built. The spec also forbids silently
assuming the first adapter is the RTX (§9.6) and forbids silent fallback to
another backend (§9.11).

## Decision

1. **Dependency:** `wgpu 27.0.1` (workspace `Cargo.toml`), dev-active in
   `anim_test_support` probe binaries; production ownership moves to
   `anim_render` at WP06 (spec §9.10).
2. **Backend:** the machine's NVIDIA driver exposes **Vulkan** through wgpu;
   the recorded driver line is `NVIDIA 617.14`. Vulkan is the spec's assumed
   backend (§9.2 prerequisite); no silent fallback to DX12/GL/Others is
   performed anywhere.
3. **Adapter selection rule:** enumerate all adapters → rank
   Discrete > Integrated > Virtual > CPU/Other → report the chosen adapter
   with its backend, driver, vendor/device IDs in every probe report. (Exact
   LUID-based DXGI correlation is a separate probe, `wp00_adapter`.)
4. **Feature gating:** devices are requested with `Features::empty()` +
   downlevel limits — only capabilities actually required, per spec §9.5.
   Format capability comes from `adapter.get_texture_format_features()`, not
   from creation side effects (spec §9.11: never report more than verified).
5. **Probe report contract:** one machine-readable JSON document per probe
   (`schema_version`, `capability`, `success`, diagnostics, selected adapter)
   on stdout; exit 0 only when required capabilities verified.

## Verified facts on the reference machine (2026-11-04)

- 6 adapters enumerated (Intel iGPU ×2 backends, RTX 3050 A, Basic Render).
- Selected: `NVIDIA GeForce RTX 3050 A Laptop GPU`, backend Vulkan, discrete.
- `max_buffer_size` = 268,435,456 (256 MiB) at downlevel limits.
- WGSL compute → one 16-lane workgroup → `COPY_SRC→COPY_DST→MAP_READ` chain
  returned **exactly** the known-integer pattern (spec §9.7 verified=true).
- `Rgba16Float` attachment capability = true; storage usage also allowed by
  the driver (recorded in diagnostics, not claimed as a requirement).

## Consequences

- Render-graph work (WP06) may assume Vulkan readback paths work on this
  machine and must revalidate this report when the driver changes (spec
  §9.12 G0: reports are invalid for a different adapter/driver).
- Probe binaries stay in `anim_test_support`; no production crate may import
  them (spec §9.10).
