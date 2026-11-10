# ADR-0011 — WP07 DXGI-Budget-Driven GPU Memory Management

**Status**: Accepted
**Date**: 2026-11-08
**Work package**: WP07 (spec §16, extract lines 824–889)
**Supersedes**: None
**Related**: ADR-0003 (adapter LUID/budget correlation), ADR-0010 (WP06 forward render graph)

## Problem

WP06 shipped a fully functional render graph but no GPU memory management. The
engine could allocate render targets without any budget awareness, eviction
policy, or out-of-memory recovery. Under competing processes or driver-managed
paging, the engine could exhaust shared memory with no diagnostic and no
recovery path. Spec §16 requires allocation accounting, DXGI budget telemetry,
residency priorities, safe eviction, bounded streaming, and OOM recovery.

## Decisions

### D1 — DXGI is advisory; internal accounting is authoritative

DXGI `QueryVideoMemoryInfo` provides an observed budget and current usage, but
competing-process attribution is a *diagnostic* only. The engine's internal
registry (sum of resident record bytes) is the authoritative size claim. When
the two disagree, the difference is logged, never acted on as proof of foreign
ownership (§16.7: "differences are diagnostics only").

### D2 — Three-tier telemetry availability

1. **Correlated adapter** (LUID + vendor/device cross-check per ADR-0003):
   real DXGI budget and usage queries.
2. **Resolved identity without live source** (IDXGIAdapter3 cast failed):
   dedicated-video-memory estimate as budget, usage = 0 (conservative).
3. **No correlation**: budget floor = accounted + reservations (self-describing),
   fraction forced to 0.60 (conservative fallback direction — §16.6).

The engine **never requests or selects a GPU** (D-11). It only correlates the
caller's device.

### D3 — Correlation strategy (ADR-0003 re-confirmed)

- Explicit 64-bit LUID hint: all-64-bit match + vendor/device cross-check.
  One mismatch → `None`, never substitute.
- No LUID: unique match on (vendor, device, description-fuzzy). Ambiguity
  (>1) → `None`.
- `correlate()` is `pub` (pure, testable) and returns `Option<AdapterIdentity>`.

### D4 — Soft target formula (pinned, f32 artifact documented)

```
fraction_part = floor(budget × target_fraction)  // f32 cast
reserve_part  = budget − safety_reserve            // saturating
soft_target   = min(fraction_part, reserve_part)   // saturating
release_watermark = soft_target − release_band     // saturating
```

The f32 multiply of 0.8 × 4 GiB = 3435973888 (a 51-byte rounding artifact of
0.8f32 = 0.8000000119…). This is a **pinned golden value** — changing it is
a golden event.

### D5 — Effective usage = max(observed, accounted, reservations)

Conservative: never double-commit the same bytes. Reservations (uncommitted
preflights) count against the target so two callers cannot be promised the
same memory.

### D6 — Eviction candidate ordering

1. Source class: Disk(0) < Regenerable(1) < CpuShadow(2) < GpuOnlyWithRestore(MAX-1)
2. Priority rank: OptionalHistory(0) … Critical(4) (least valuable first)
3. Oldest `last_used_serial` (settled only: `last_used ≤ completed`)
4. Lowest byte cost (reload cost proxy)
5. Stable `ResourceId` tie-break

Excluded: locked, mapped, required, unrestorable GPU-only, mid-reload, already-released.

### D7 — Hysteresis: 3 consecutive over-budget samples latch eviction

- Latch engages after `over_budget_samples` (default 3) consecutive samples
  where `effective_usage > soft_target`.
- Once latched, eviction continues below the **release watermark** (lower band).
- Full reset only when `effective_usage ≤ release_watermark`.
- Actual allocation failure bypasses sampling delays entirely.

### D8 — Reservation ledger: Rc<RefCell> shared, drop-safe

- `AllocationPermit` carries a shared ledger handle; `Drop` releases the
  reservation unless `commit()` was called.
- `commit()` consumes the ledger entry exactly once; subsequent drop is a no-op.
- `MAX_ALLOCATION_RETRIES = 1` (bounded extra eviction before a single retry).

### D9 — Preflight outcomes

- `Grant`: projection ≤ soft target.
- `GrantAfterEviction`: satisfied plan brings projection under target.
- `Refuse`: actionable OOM with requested/budget/effective/reservation/shortfall.

Releases never eat reservations in projection accounting.

### D10 — Device loss invalidates all serial assumptions

Serials reset to 0; residency records flip to `PendingReload` (except
unrestorable `GpuOnly` which stays dead); uncommitted reservations cleared;
content rebuilds from CPU/disk via the reload queue.

### D11 — Preview degradation: unlocked OptionalHistory + Cache only

Locked records are never touched. Degradation drops degradable classes
regardless of hysteresis.

### D12 — Settings validation (anim_app)

- `target_fraction` ∈ (0, 1]
- `over_budget_samples` ≥ 1
- `max_upload_bytes_per_frame` > 0
- `release_band_bytes` must not collapse the watermark (4×-gap heuristic)

### D13 — Synthetic test hook (doc(hidden))

`MemoryManager::synthetic: Option<SyntheticBudget>` — the §16.3 test_support
contract. Injects budget/usage and a fault-injection counter. The manager
treats synthetic presence as telemetry-available (so the conservative 0.60
fraction does not silently apply to harness tests).

### D14 — Queue allocation accounting

`MemoryManager` owns a `ReservationLedger` (Rc<RefCell>). The `BudgetTelemetry`
trait abstracts the DXGI source. `query_local_memory` checks interval, calls
the source (or falls back), computes effective usage, and stores an immutable
snapshot.

### D15 — Fence lifecycle

- One serial assigned at every `queue.submit`.
- `mark_used(id, serial, frame)` records highest referencing serial.
- Eviction requires `last_used_serial ≤ completed_serial` (in-flight exclusion).
- `on_submitted` / `on_completed` callbacks.

### D16 — Reload streaming

- `ReloadBatch { scheduled, deferred, backed_up }` — bounded by frame budget.
- Oversize entries deferred whole (never truncated, never dropped).
- Best-fit FIFO (no head-of-line blocking): a deferred entry does not block
  later entries that fit.
- Resident entries cost nothing and never defer.

### D17 — Readback staging

Registered as a locked resource during map. In chunk 2, the staging buffer is
registered at renderer creation and locked for the lifetime of the renderer
(MVP simplification; G6 locked-selection tests cover the exclusion).

## Consequences

- The memory module compiles clean with 92 unit tests + 4 harness tests.
- Real DXGI telemetry works on RTX 3050 (budget=3308 MiB, VRAM=3892 MiB).
- The render graph (WP06) now has budget-aware resource tracking.
- G6 tests (chunk 3) validate all spec §16.12 scenarios.
- Chunk 2 device.rs wires fence serials per submission; chunk 3 adds
  preflight integration with real wgpu allocation failures.

## Alternatives considered

- **Automatic GPU selection**: rejected (D-11). Engine only correlates.
- **DXGI as authoritative**: rejected — competing processes make DXGI usage
  non-attributable; internal accounting is the only honest claim.
- **Hardcoded telemetry fraction**: rejected — conservative fallback must be
  a function of telemetry availability, not a constant.
- **Unregister on resize**: rejected — keeping dead records is harmless
  (resident sum excludes them) and spec §16.8 mutation list does not include
  unregistration.

## Evidence

- Live budget_report on RTX 3050 A Laptop GPU: LUID=0x11a4e, VRAM=3892 MiB,
  budget=3308 MiB, tele=true, accounted=196896 bytes (9 resources).
- 92 anim_render lib tests + 4 harness tests all pass.
- Full workspace gates: fmt=0, clippy=0 warnings, test --workspace --locked=0.
- WP06 goldens 8/8 re-verified in the same run.

## ADR numbering note

The spec's ADR register (§24) maps ADR-0011 to "Render pass and post-effect
order" (WP06) and ADR-0012 to "DXGI policy defaults and unavailable-telemetry
behavior" (WP07). This ADR is filed as **ADR-0011** following the engine's
sequential numbering (ADR-0010 = WP06). The spec's ADR-0012 content is
absorbed here; the WP06 post-effect order decisions live in ADR-0010.
