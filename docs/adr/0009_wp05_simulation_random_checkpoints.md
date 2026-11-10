# ADR-0009 — WP05: Deterministic Simulation, Philox Random, Checkpoints & Seeking

**Status:** Accepted (2026-11-07) · **Scope:** `anim_sim` + `anim_core`/`anim_project` additions · **Spec:** §14

## Context

WP05 adds the deterministic fixed-step simulator: frame-locked ticks, exact
per-tick state hashing, address-based Philox4x32-10 random draws, canonical
event schedule, particle + spring systems, checkpoint cache with cadence,
randomized seek replay and the two-process G4 gate (§14.12). This ADR records
each decision so no chat context is load-bearing.

## Decisions

1. **Dependency deviation (recorded per plan):** `anim_sim` depends on
   `anim_project` (read-only) IN ADDITION to the §14.4 set
   {anim_core, anim_scene, read-only anim_assets}. Rationale: `TickSchedule`
   compiles from the AUTHORITATIVE document (the compiled schedule is the
   only runtime event source), and initial state reads emitter/spring
   authoring from `SceneData`. No crate depends back on anim_sim; the cycle
   risk anim_scene→anim_project→anim_sim→anim_project does not exist.

2. **Philox constants lesson (memory vs reference).** The multiplier
   `M1 = 0xCD9E8D57` (NOT 0xCD9E8D05, which was mis-remembered from training
   data and FAILED the published KAT). Constants verified against the cloned
   reference `Random123/include/Random123/Philox.h`
   (`PHILOX_M4x32_1`): M0=0xD2511F53, M1=0xCD9E8D57, W0=0x9E3779B9,
   W1=0xBB67AE85, ROUNDS=10, key bump AFTER each round application. The
   published Random123 KAT vectors (all-zero, all-max, π counter) pass in
   `random.rs` unit tests. LESSON: generator constants are KAT-gated, never
   trusted from memory.

3. **Cursor-free draws.** No mutable RNG object exists. Every draw =
   Philox(counter=(tick_lo, tick_hi, draw_index, 0), key=(u64 split LE into
   two u32 words)). Stream keys = BLAKE3 over
   `anim_engine/philox/v1` ∥ seed ∥ u16 system ∥ uuid(entity) ∥ u16 purpose ∥
   u32 version. f32 = upper 24 bits / 2^24. Purposes: EMITTER_RATE=0,
   PARTICLE_SPAWN=1. Channel table (pinned in `random.rs::channel`):
   ANGLE=0, SPEED=1, LIFETIME_BAND=2, SIZE_BAND=3, ROTATION=4, JITTER_X/Y/Z=5/6/7,
   SPAN=8 (one address block per spawn ordinal; rate-decision draws live in
   a different stream).

4. **Emission-accumulator spawn mechanic (MVP).** Per emitter, an EXACT
   Rational accumulator advances by `spawn_rate·dt` (rate converted f64→
   exact BigInt fraction via IEEE-754 bit decomposition; dt = rate.den /
   rate.num). Integer part spawns deterministically; a Philox rate-decision
   draw at p = frac adopts ONE extra particle; the ADOPTED count
   (int + adoption) is subtracted exactly. Accumulators hash into
   checkpoints (`emission` map), so cadence never changes spawns. MVP
   limitation: emitters are positioned from the FRAME-0 snapshot origin
   (mid-run emitter motion not tracked).

5. **Particle identity & stability.** id = BLAKE3
   (`anim_sim/particle_id_v1`) over (NodeId emitter ∥ u32 generation ∥ u32
   ordinal) → first 16 bytes as LE u128. The particles vector is kept
   SORTED by id at every observable moment (sorted `partition_point`
   insert); removal is a stable filter, so surviving order is preserved —
   insertion/removal stability is test-enforced.

6. **Particles: semi-implicit Euler** (`v += a·dt; x += v·dt`), rendered
   pairs from `previous_position`. Ages advance every tick (spawn tick
   included); death when `age == lifetime` (stable removal). Spring bodies:
   gravity into velocity, then a FIXED `solver_iterations` count of
   Gauss-Seidel edge relaxations (sequential, canonical edge-id order,
   per-pass factor `k/(1+k)`), then ONE position integration. Uniform
   `inverse_mass = 1.0` (MVP). Springs map stiffness ∞→k/(1+k)<1 to keep
   relaxation stable; authored `stiffness = 0` is an inert edge.

7. **Mode is policy, never numerics.** `SimulationMode::{Live, Replay,
   WarmUp, Export}` changes ONLY checkpoint capture: Live/Replay follow the
   cache cadence; Export additionally captures at EVERY frame boundary;
   WarmUp captures nothing. The solver path (`step_once`) is identical in
   all modes — chunked/single, checkpointed/uncheckpointed and export runs
   hash-identical (tests enforce).

8. **Checkpoints.** `Checkpoint` carries algorithm/input versions, the
   sorted particles/springs/bodies, exact `emission` accumulators,
   generation/ordinal maps, event cursor, gravity bits AND a capture-time
   hash (`verify_checkpoint` recomputes from fields → any field corruption
   is detected; corrupted entries are discarded NEWEST-WITH-SUFFIX during
   seek verification). Encoding: LE integers, f32 bit patterns, sorted
   collections; domain `anim_sim/checkpoint_v1`. The state hash EXCLUDES
   the clock (configuration is revalidated from context at restore).
   `CheckpointCache` = (cadence_ticks, capacity) with oldest-evict;
   cadence 0 disables capture entirely (seek falls back to full replay).

9. **Seek = greatest verified checkpoint ≤ target, else replay from tick 0,
   through the SAME `step_once`** with an explicit replay budget
   (`ReplayBudgetExhausted`); overshoot is a `MalformedCheckpoint` error,
   not a silent extra step. Seek to tick 0 restores the pristine initial
   state (test-enforced against a fresh context).

10. **anim_core additions.** `SpringEdgeId` ("springedge" typed id);
    `CoreError::Validation(String)`; `anim_core::sim_config` carries
    `SimulationSettings`, `EmitterSettings`, `SpringEdge` (shared because
    BOTH anim_project documents and anim_sim need them without a cycle);
    `Rational` gained `PartialOrd/Ord` support where needed by WP04.
    `anim_project` gained `SceneData.emitters/springs` per §14.2 with
    `serde(default)` (WP03-era documents still load).

11. **Event schedule = flat compiled artifact.** `TickSchedule::compile`
    expands every definition occurrence (times are exact Rationals; tick =
    ceil(t·rate)) into ONE sorted Vec keyed by the CANONICAL event order
    (tick, priority, source, local_seq, occurrence_id BLAKE3 first 16 bytes
    of `anim_engine/event_occurrence_v1`). `SIMULATION_INPUT_VERSION = 1`
    is exported and checkpointed. In MVP, Spawn events DO NOT alter state
    (the rate accumulator is the spawn mechanic — decision 4); Cue/Note
    consume cursor only.

12. **Headless wiring deferred.** Spec §9.3 pins real `anim_headless`
    behavior at WP09/WP10, so the WP05 plan's "replay module wiring" is
    intentionally NOT implemented now; anim_headless remains the WP00 stub
    and replay integration lands with WP09. anim_test_support remains
    build-only for WP05 (its determinism helpers were placed directly in
    `anim_sim/tests`).

## Verification (gates, all green 2026-11-07)

- `cargo fmt --all` → clean; `cargo clippy --workspace --all-targets` → 0
  warnings; `cargo test --workspace --locked` → 0 failures.
- `anim_sim` tests: 6 lib (Philox KAT, cursor-free/lane/range, stream
  versioning, clock exactness) + 8 `determinism.rs` (cadence matrix
  {1,7,64,1024,disabled} × randomized in/off-cadence seek-vs-golden hashes;
  seek-order independence; chunked-vs-single; export==uncheckpointed;
  nonfinite→`SimError::NonfiniteState`; 400-tick churn insertion stability;
  seek-to-0 purity; clock plumbing) + 1 `cross_process.rs` (G4: two fresh
  `g4_probe` processes byte-identical over interleaved seek targets).

## Consequences

- Any change to constants/domains/channels/sort keys is a VERSION bump (the
  checkpoints and hashes invalidate BY DESIGN).
- MVP limits documented here (frame-0 emitter origins, uniform mass,
  no-drag particles, event-Spawn-as-no-op) are the WP06+ surface.
