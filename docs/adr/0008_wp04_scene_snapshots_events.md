# ADR-0008 — WP04: Evaluated Scene Snapshots & Canonical Events

**Status:** Accepted (2026-11-06) · **Scope:** `anim_scene` + `anim_core`/`anim_project` additions · **Spec:** §13

## Context

WP04 turns authoritative project data into immutable, revision-bound evaluated
snapshots shared read-only by simulation, rendering, audio, editor and export
(spec §13). This ADR records every spec-interpretation and MVP decision so no
chat context is load-bearing.

## Decisions

1. **Dependency direction.** `anim_scene → anim_project → anim_assets →
   anim_core`. Consumers never appear in `anim_scene` dependencies, so a later
   simulator/renderer cannot mutate an authoritative document through a
   snapshot back-door. The dependency goes ONE way (spec §13.4). This forces
   the event taxonomy (`EventKind::{Spawn, Cue, Note}`) and `EventDefinition`
   to live in `anim_project` (raw scene data), re-exported by `anim_scene` —
   NOT the reverse, which would create a cycle.

2. **Canonical property set** (evaluated per node at every position; names
   pinned here because snapshots define behavior downstream):
   `translation_x` (0), `translation_y` (0), `rotation` (0 radians),
   `scale_x` (1), `scale_y` (1), `z` (0), `opacity` (1, clamped [0,1]),
   `visible` (Bool true), `camera` (Bool false; static ROLE via track
   default, not time-varying), `camera_zoom` (1), `cel` (Int 0; sprite-cel
   index). A node track with NO default and empty keys is a typed
   `InvalidPropertyKeys` error (never silent 0).

3. **Local transform composition** = T·R·S (translate·rotate·scale) in one
   matrix `[[cos·sx, −sin·sy, tx], [sin·sx, cos·sy, ty], [0,0,1]]`;
   world = parent_world · local evaluated in a validated traversal (roots in
   NodeId order, children in persisted sibling order — both BTreeMap/NodeId
   ordered, so "persisted order" is byte-canonical and serializable).

4. **Easing version `EASING_ALGORITHM_VERSION = 1`.** Exact rational interval
   fraction `u = (t−t0)/(t1−t0)`, converted to f64 ONCE; Linear = u;
   Smooth = u²(3−2u); Step = preceding key; Bool/Text tracks may only differ
   between keys under Step (else `InvalidEasing`). Exact key time returns
   THAT key; outside the key span the first/last value holds.

5. **Layer blending (MVP: override).** Multiple `TrackData` targeting the same
   (node, property) sort by `layer`; the highest layer REPLACES the node's
   own property evaluation WHEN that track carries keys inside its span (the
   keys live on the target node). A layer track whose target node has no keys
   for that property contributes nothing — not an implicit zero (proven by
   `tracks_layer_override_applies`). Numeric layer MIXING is deferred; a
   version bump of `EASING_ALGORITHM_VERSION` is REQUIRED if it lands.

6. **Cameras & cuts.** A camera is any node whose `camera` track DEFAULT is
   Bool(true). `MarkerKind::Cut` markers NAME a camera node BY NODE NAME; at
   position p the active camera is the one named by the LATEST cut with
   `at ≤ p` (cut list pre-sorted by time); before any cut → the lowest-NodeId
   camera. A cut naming an absent or non-camera node is a TYPED
   `SceneError::UnknownCamera` rejection at evaluation (§13.11) — never a
   silent skip.

7. **Event model.** `EventDefinition { name, kind, priority, source, asset?,
   times: Vec<Rational>, params }` lives in the authoritative document with
   invariants checked at document validation AND re-checked on the evaluation
   path (strictly increasing times; source node exists; Cue assets exist in
   the manifest). `occurrence_index` = index into `times` (stable SOURCE key,
   never enumeration order). `EventOccurrenceId = BLAKE3("anim_engine/
   event_occurrence_v1" ∥ EventDefinitionId bytes ∥ u32 index LE)[..16]` —
   derived once, deterministic, injective across (definition, index).

8. **Frame ownership (delivery window).** Frame n owns events with exact time
   t satisfying time_of(n) ≤ t < time_of(n+1) (upper boundary belongs to the
   NEXT frame). `SceneEvent.time` is STAMPED at the containing frame's start;
   the exact authored rational survives in `EventDefinition::times` and in
   `SimulationInput::at` for consumers that need it. Frame ownership never
   depends on subframe evaluation order.

9. **Canonical event order** =
   (event_time, priority, source_node_id, local_sequence, occurrence_id).
   UUID ties compare bytewise. The comparator is total and free of
   HashMap/enumeration state — proven by the shuffle test.

10. **Canonical hashing.** `snapshot_hash` domain `"anim_scene_snapshot_v1"`,
    `event-log digest` domain `"anim_scene_events_v1"`; encoding is
    declaration-order, little-endian, length-framed collections in BTree/Vec
    canonical order, f64 as BIT PATTERNS after finite validation
    (`InvalidNumericValue` on nonfinite). Event kinds tag as Spawn=0, Cue=1,
    Note=2 (pinned; reordering is a breaking spec change).

11. **Cache.** `SceneCache` keys on (SceneId, Revision, FramePos), bounded at
    128 entries with oldest-insertion eviction; a stale revision can be
    fetched ONLY by its key and can never impersonate the current revision;
    `retain_only_revision` prunes on undo/redo. Evaluation itself NEVER
    touches the cache (statelessness is the function's property, not the
    cache's).

12. **G3 re-exec proof.** `anim_scene::bin/g3_probe` evaluates ONE
    (document, scene, frame) from a saved project file and prints the
    snapshot hash + event digest. The integration test
    `two_fresh_processes_agree_on_snapshot_and_event_digests` spawns the
    compiled probe binary TWICE (different env tags) and demands byte-equal
    stdout. Two processes agreeing is THE acceptance gate (§13.12).

## anim_core / anim_project additions

* `anim_core::EventOccurrenceId` (typed UUID wrapper, domain "eventocc").
* `FramePos` gains `PartialOrd`/`Ord` derived over (frame, subframe) — the
  event sort key and Cut-marker comparison need exact total ordering; the
  field order = declaration order makes the derive mathematically correct.
* `ProjectDocument::scenes[*].events: BTreeMap<EventDefinitionId,
  EventDefinition>` with `serde(default)` so WP03 documents load unchanged.
* `SceneData::validate_event_definitions(&manifest)` public: source node
  exists, Cue asset in manifest, times strictly increasing, params finite.
  Wired into document schema validation AND the evaluation path.

## Consequences

* Simulation (WP05) consumes `SceneSnapshot::simulation_inputs` plus the
  event log; audio (WP08) converts `AudioCue`s at *snapshots-as-delivered*
  to exact sample positions — conversions never happen inside `anim_scene`.
* Any future pose/skin/drawing feature extends `EvaluatedNode` additively;
  hashing field order is pinned, so additions APPEND (append-old-field-
  first), never reorder.
* The `g3_probe` binary is part of the crate's public test surface; keep it
  compiling (it is the gate artifact for the Phase 2 user demo too).
