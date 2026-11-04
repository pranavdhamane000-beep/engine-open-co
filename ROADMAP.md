# Execution Roadmap — Animation Engine (5 Phases)

_Normative source: `../spec_extract.txt` (DES v3.2). This file = OUR slice order + session estimates + per-phase checkpoints. The spec remains authoritative when the two differ._

**Update protocol:** after each session → update STATUS.md; after each decision → append DECISIONS.md; a phase is "Done" only when its exit gate row in STATUS.md records a PASS.

---

## Dependency map (from spec §6.1)
```
WP00 → WP01 → WP02 → WP03 → WP04 → WP05 → WP06 → WP07 → WP08 → WP09 → WP10 → WP11
                                                (WP08 may parallel WP06/07 after WP04)
WP12 → WP13 → WP15 → WP14 → WP18 ∥ WP19 → WP20 → WP21 → WP22          (Part B + Level 3)
```

## Phase table

### Phase 0 — Bootstrap (foundation hygiene)
| # | Task | Sessions | Proof of completion |
|---|---|---|---|
| 0.1 | Install rustup + MSVC toolchain; verify `cargo --version`; create `CARGO_TARGET_DIR` redirect | 1 | `cargo test` green on a hello-crate |
| 0.2 | `git init`, first commit of these md files + spec extract | 1 | commit hash recorded in STATUS.md |

### Phase 1 — Ground & Core Data (WP00–WP02)
| # | WP | Deliverable | Exit proof |
|---|---|---|---|
| 1.1 | WP00 | 13-crate workspace per spec §4.1; rust-toolchain pinned; locked deps; risk prototypes | workspace builds; ADR-0001 written |
| 1.2 | WP01 | Rational time (`FramePos`), typed IDs, canonical hashing, atomic file replace | property tests: time round-trips, hash invariance |
| 1.3 | WP02 | Immutable asset store, importer-versioned derived caches, manifest commands | save/reopen + asset-corruption fault tests green |

**User checkpoint after Phase 1** — demo: create project → import asset → save → hash → reopen → hash identical.

### Phase 2 — Commands & Deterministic Motion (WP03–WP05)
| # | WP | Deliverable | Exit proof |
|---|---|---|---|
| 2.1 | WP03 | Command envelopes, registry, transactions, undo/redo, crash-safe saves | interrupted-write fault tests |
| 2.2 | WP04 | Immutable SceneSnapshot, canonical event ordering, cameras | snapshot-immutability tests |
| 2.3 | WP05 | Fixed-step clock, seeded random streams, seeking, checkpoints | **10,000-frame × 2 fresh processes → identical state hash** |

**User checkpoint after Phase 2** — demo: scrub/seek a simulated scene, restore from checkpoint, prove replay equality.

### Phase 3 — Output Pipeline (WP06–WP09)
| # | WP | Deliverable | Exit proof |
|---|---|---|---|
| 3.1 | WP06 | Linear-HDR forward render graph, alpha, motion vectors | golden color-patch + alpha devices |
| 3.2 | WP07 | DXGI-budget GPU memory manager, eviction policy | memory-pressure fault tests |
| 3.3 | WP08 | Deterministic offline 48 kHz audio mix + preview clock | PCM golden tests (pre-codec) |
| 3.4 | WP09 | Headless export: resolved jobs, FFmpeg, verified publication | small codec-matrix exports verified by decode metadata |

🏆 **Milestone demo for user:** render the Arena Ep1 choreography data through the new engine's render graph → compare against Python MVP output.

### Phase 4 — Editor & Hardening (WP10–WP11)
| # | WP | Deliverable | Exit proof |
|---|---|---|---|
| 4.1 | WP10 | egui editor shell: panels, transport, pen strokes, preview quality | editor never mutates scene storage directly (boundary test) |
| 4.2 | WP11 | Integration tests, benchmarks (benchmark scene + results recorded), delivery scripts (verify_release.ps1) | long-export run with bounded memory; benchmark numbers in docs/benchmarks/ |

**Exit gate:** spec §25 "Final Definition of Done" for Part A. **Phase 4 = the foundation is finished.**

### Phase 5 — Character, Combat, Camera (WP12–WP22, spec Part B + Level 3)
| Slice | WPs | Milestone/gate |
|---|---|---|
| 5a | WP12 (rig IK/FK) → WP13 (weapons/combat) | **M1+M2** — hardest chain; Test A pose/IK proof, Test C/D contact proof (G10–G11) |
| 5b | WP15 (authored camera) → WP14 (secondary motion/VFX) | **M3+M4** — Test E (G12–G13) |
| 5c | WP16 (action-aware auto-camera) → WP17 (advanced camera) | **M5+M6** — planner + no-drift/collision/trauma proofs (G14–G15) |
| 5d | WP18 (hands/grips) ∥ WP19 (face/acting) | G16–G17 |
| 5e | WP20 (action library) → WP21 (dope sheet/X-sheet tools) | G18–G19 |
| 5f | WP22 (Level 3 hardware envelope) | G20 — telemetry on RTX 3050A at 1080p envelope |

**Final acceptance:** one Level-3 action scene with readable anticipation → contact → follow-through → recovery (spec §39.5).

---

## Estimated cadence (honest)
- Phases 0–1: toolchain + scaffolding = quick wins, ~3–6 sessions
- Phases 2–3: the deterministic-engineering core = the majority of Part A effort
- Phase 4: hardening, mostly test-writing
- Phase 5: longest (M1/M2 rig+combat is the known crunch); each slice independently checkpointed
- Cadence is session-based, not calendar-promised; STATUS.md logs actuals

## Build hygiene (Phase 0 setup, per D-04)
- `CARGO_TARGET_DIR` → `C:\Users\Pranav\AppData\Local\anim_engine_target` (multi-GB, out of OneDrive)
- OneDrive keeps: source code, docs, schemas, tests, golden fixtures (small files only)
- Verify `.gitignore` excludes nothing needed; commit small golden files only

## Work rules (carried from spec §6.2–6.3, enforced every phase)
- No floating dependency versions; WP00 locks everything
- No temporary debug behavior across architecture boundaries
- Every contract change = ADR + versioned schema + migration tests
- Gate results (pass/fail + evidence) recorded in STATUS.md, never assumed
