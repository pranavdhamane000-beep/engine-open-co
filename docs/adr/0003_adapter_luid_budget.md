# ADR-0003 — Vulkan/DXGI LUID correlation and the memory-budget baseline

**Status:** Accepted · **Date:** 2026-11-04 · **Evidence:** `artifacts/probe/wp00_adapter_report.json`

## Context

Spec §9.7 "Adapter correlation" requires matching the Vulkan physical device
to its `IDXGIAdapter1` by **all 64 LUID bits**, cross-checked with
vendor/device IDs, with the unavailable path reported (`IdentityUnavailable`)
and **never** substituted by the primary desktop adapter. §9.5 step 6 adds a
DXGI budget query. wgpu 27.0.1 does **not** expose Vulkan physical-device LUID
anywhere in its public API (checked `wgpu-types` + `wgpu-hal` sources), so the
probe queries Vulkan directly via `ash 0.38` (the same ash version wgpu-hal
already pins — one copy in the tree).

## Decision

1. **ash instance setup:** `API_VERSION_1_3` explicitly set **plus** the three
   core-promoted capability extensions enabled
   (`VK_KHR_external_memory_capabilities`, `…_semaphore_…`, `…_fence_…`).
2. **Machine fact (load-bearing):** without that capability surface, the
   NVIDIA **and** Intel Windows ICDs report `deviceLUIDValid = VK_FALSE` even
   though the Vulkan spec does not condition the field on extension enabling.
   With the surface present at API 1.3, both ICDs returned valid LUIDs and
   exact 64-bit DXGI matches. Any future probe that needs LUIDs MUST reuse
   this instance configuration.
3. **Correlation rule (in code, not prose):** a Vulkan device with a valid
   LUID matches the DXGI adapter ONLY on (luid equal bit-for-bit) AND
   (vendor + device IDs equal). On any miss → `IdentityUnavailable` with the
   reason recorded; the negative test (`luid ^ 0x5555…`) proves a mismatched
   LUID can match nothing — substitution is structurally impossible.
4. **Budget baseline:** `IDXGIAdapter3::QueryVideoMemoryInfo(0, LOCAL, …)`
   feeds the spec §9.8 `BudgetSnapshot` fields. `accounted_usage` stays at 0
   until WP07 starts accounting engine allocations.

## Verified facts on the reference machine (2026-11-04, driver 617.14)

- 2 Vulkan physical devices; both `luid valid` after capability exts:
  - `Intel(R) Graphics` → luid 69483, DXGI match ids_ok = true
  - `NVIDIA GeForce RTX 3050 A Laptop GPU` → luid 123521792, DXGI match
    ids_ok = true, dedicated video memory 4,081,057,792 B
- `Microsoft Basic Render Driver` enumerated in DXGI without Vulkan
  counterpart (correct — CPU/warp device).
- Budget snapshot (NVIDIA): **budget = 3,468,899,124 B** (~3.23 GiB) of the
  4 GB card — the reference number for the WP07 memory-policy target fraction.

## Consequences

- WP07 (GPU memory manager) may rely on LUID correlation for its adapter
  selection and on `|budget ≈ 3.23 GiB|` for the 4 GB envelope test, with
  revalidation on driver/OS change (spec §9.12 G0 rule).
- The probe is re-runnable: `cargo run -p anim_test_support --bin wp00_adapter`.
- `anim_render` will own production Vulkan/DXGI correlation; the probe stays
  test-only (spec §9.10).
