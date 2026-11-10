# ADR-0010 — WP06: Forward Render Graph, Pinned Post Chain & G5 Goldens

**Status:** Accepted (2026-11-08) · **Scope:** `anim_render` + small `anim_core`/`anim_sim`/`anim_scene` additions · **Spec:** §15

## Context

WP06 implements the spec §15 forward render graph end-to-end: pure conventions
(chunk 1: errors/alpha/camera/draw_list/targets/graph), the GPU half (chunk 2:
5 WGSL shaders, instance upload, passes, readback, `render_scene`) and the G5
goldens (chunk 3: pinned-stack readback vs CPU reference math, verified live on
RTX 3050, 2026-11-08). This ADR records every decision, deviation and
root-cause lesson so no chat context is load-bearing.

## Decisions

1. **Dependency deviation:** `anim_render` additionally depends on `anim_sim`
   (read-only) beyond the §15.4 set. Rationale: `extract_frame` merges
   `SceneSnapshot` + `SimulationFrame` (body transforms, particles) into the
   sorted `RenderFrame`. No crate depends back on anim_render.

2. **2D camera convention pinned** (`camera.rs`): ortho looking +z, y-up,
   `VIEW_HEIGHT_WORLD = 10.0`, NEAR/FAR ∓1000 → depth ∈ [0,1] via `_rh`
   matrices; no-camera default = identity. Both current and previous VP are
   built by `CameraMatrices::eval2d` from ONE evaluated camera; on the first
   frame after a cut `previous == current` (velocity = 0, never garbage).

3. **`DEPTH_QUANTUM_SCALE = 64`** for the transparent sort key
   (view_depth → i32); `PARTICLE_LAYER_ORDER = i32::MAX / 4` reserves the top
   quarter for particles without colliding with authored i32 layers. Draw
   order = stable total sort: layer ASC, quantized depth DESC, material, then
   entity + draw_index (duplicates detected BEFORE sort → typed
   `DuplicateSortIdentity`).

4. **`DrawItem` additive fields (spec deviation, documented):**
   `opacity: f32`, `tint: [f32; 4]` (straight-authored linear; premul items
   carry ALREADY-premultiplied tints), `velocity_valid: bool`. These let one
   struct feed every pass without per-pass carrier types. `RenderFrame`
   additionally carries `camera_id: Option<CameraId>` for WP07 diagnostics —
   §15 does not name either field.

5. **Alpha table pinned (`instance.rs::alpha_kind`):** flags.x = 0.0
   alpha-test (cutoff flags.y, opaque MRT only), 1.0 straight (shader decodes
   rgb·a), 2.0 premultipled passthrough, 3.0 additive (pipeline blend One/One).
   GPU blend: over = One/OneMinusSrcAlpha for color AND alpha (alpha rules are
   per-component, never approximated); blended passes use LessEqual with
   depth-write OFF so coplanar ties both draw.

6. **Velocity contract:** screen-space uv deltas per §15.7
   (`ndc_to_uv(cur_clip.xy / w) - ndc_to_uv(prev_clip.xy / pw)`), validity in
   z ≥ 0, reserved w = 0. Per-item `velocity_valid: false`, previous w ≤ 0, or
   a cut frame → no entries (target stays cleared). Storage f16 ⇒ deltas
   quantize (e.g. 0.15 → 0.14990234); goldens use ≥ 0.01 tolerance for that
   channel only.

7. **`HistoryState.previous_tick: Option<SimTick>`** (not a tick constant):
   tick 0 is a valid previous frame. Cut validity = explicit
   `history.valid == false` bit; cut_serial bumps EXACTLY once per cut (G5
   asserts 0 → 1 across one non-cut and one cut frame). Resize with aspect
   change → ProjectionChange + TargetResize invalidation, aspect updated in
   the SAME response; the following successful render re-validates.

8. **`anim_sim` ParticleDraw extensions (deviation):** `age_ticks: u32` is
   carried so `extract_frame` can mark age ≤ 1 particles velocity-invalid
   (no trustworthy previous); plus `draw` helpers exposing size/opacity inputs
   needed by the renderer contract. anim_scene's `SimulationFrame` unchanged.

9. **Post chain = pinned 6-stage ping-pong** (`graph.rs::post_chain`,
   `device.rs::render_scene`): motion(scene+vel→P0) → base copy P0→scene_hdr →
   bright(scene_hdr→P1) → bloom_v(P1→P0) → combine(scene_hdr+P0→P1) →
   tone(bloom? P1 : scene_hdr→P0) → grade(P0→P1) → output reads P1 (P0 when
   grade off). Motion blur with a fully invalid velocity pair is an identity
   copy (no blur). Defaults (§15.7): steps=4, threshold=1.0, gain=1.0,
   radius=1, strength=1.0. **THE PINNED CHAIN MODEL (G5-enforced):**
   `out = transfer(clamp(tone_map(chain_color)))` with Reinhard tone map
   ALWAYS applied; grade clamps RGB at neutral (alpha untouched); transfer =
   hardware (sRGB target) or manual encode — exactly once either way.

10. **MVP "bloom-lite" documented refinement note:** the bright pass emits
    `c.rgb · gain` per tap when `lum > 1.0`, so a UNIFORM bright quad's bloom
    layer == its base color and the combine DOUBLES it (e.g. tint 4.0 → tone
    input 8.0). The G5 HDR golden encodes this as `uniform_bloom(base)`
    rather than pretending a gaussian pyramid exists. Full separable gaussian
    is a declared WP07+ refinement; nothing was silently faked.

11. **HDR safety margin:** the HDR golden uses tint [4.0, 0.5, 0.1]
    (lum = 1.215) — an earlier draft used [3.0, 0.5, 0.1] whose lum = 1.0026
    sits on the bloom threshold edge (GPU gain changes flip it). Tests must
    clear thresholds with margin, not sit on them.

12. **`InternalTarget` diagnostics API:** `Renderer::readback_internal`
    exposes every intermediate (SceneHdr, Normal, Velocity, Post0, Post1)
    through the same padded-row readback path as production so goldens can
    pin PRE-post values (e.g. f16 r = 4.0 surviving unclamped before tone
    map). This is diagnostics-only; the render path never reads it.

13. **`pollster` is an anim_render (test-visible) dependency:** blocking
    device adoption inside `create_renderer` keeps the D-11 contract
    (caller owns device+queue, engine never picks a GPU). It is not a runtime
    global.

14. **Format probing philosophy:** `create_renderer` validates the caller
    format against the five REQUIRED internal formats via a validation-error
    scope captured into typed `MissingRequiredFormat` — no global error scope,
    no fallback formats, no silent substitution (device-lost → typed error +
    clean re-init is the WP06 add-on requirement, verified by smoke).

15. **Pass grouping beats layer order ACROSS passes (lesson, now
    spec-aligned):** the pinned MAIN_PASS_SEQUENCE (OpaqueMrt → Ordered2D →
    Transparent → PostChain → OutputTransfer) is executed verbatim and logged;
    the G5 order golden asserts the log == MAIN_PASS_SEQUENCE and that the
    LAST writer wins. Corollary surfaced by the layer golden: a TRANSPARENT
    item at layer −5 still composites over an ORDERED item at layer +5
    because pass sequence groups overrides layer_order across passes. Layer
    ordering is only relatable within one material pass — the golden and
    draw docs now say this explicitly (MVP limitation: cross-pass layered
    composition belongs to the WP07+ layered/depth model).

## Root-cause lessons (all caught by G5 goldens, all fixed)

16. **Depth `StoreOp` chain (was shipping since chunk 2):** `ordered_2d.rs`
    declared depth `store: Discard`; the transparent pass LOADs that depth
    afterwards → undefined content (observed: driver returns ≈ 0 → every
    transparent fragment z-failed → **transparent/additive drew NOTHING**).
    Rule: any pass whose depth is READ later must `Store`. The isolating
    bisect (run the transparent item through the opaque pass) plus an
    ordered-pass removal pinned the interaction in one probe session.

17. **Velocity interpolant lesson:** `@builtin(position)` xy is ALREADY raster
    PIXEL space; feeding it to `ndc_to_uv` produced garbage gradients
    (20.75 = pixel coords). Fix: a plain `@location(0) cur_clip` interpolant
    in `scene.wgsl` (prev_clip → 1, tint → 2, flags → 3); the fragment
    reconstructs NDC by `/w`. Ground-truth check: a +2-world-unit mover now
    reads dx = 0.5·(2/8) = 0.14990234 (f16 exact), validity z = 1.

18. **Manual sRGB transfer DIRECTION (was shipping since chunk 2):**
    `output.wgsl` implemented the decode (`pow((lin+0.055)/1.055, 2.4)`)
    instead of the encode → Rgba8Unorm output bytes were quadratic garbage
    (0.333 linear → byte 23). Fix: piecewise encode
    (`12.92·lin` / `1.055·lin^(1/2.4) − 0.055`); sRGB and manual paths now
    produce IDENTICAL bytes (G5 exactly-once golden), e.g. [0.5, 0.25, 0.75]
    → [156, 124, 175] on BOTH targets.

19. **Reference-math discipline:** the first G5 references were authored
    without the tone map and from a mis-set alpha authoring; every failure
    was resolved by PROBE (dump scene_hdr + post0/1 + velocity + output per
    crafted frame) and only then correcting the CPU reference — never by
    loosening assertions to match the GPU. `chain_byte(c) = srgb_byte(c/(1+c))`
    and `uniform_bloom` are the verified helpers; alpha tests author
    STRAIGHT tints and let the shader decode.

## Verification (gates, all green 2026-11-08)

- `cargo fmt --all --check` → 0; `cargo clippy --workspace --all-targets` →
  0 warnings; `cargo test --workspace --locked` → 0 failures (exit 0).
- `anim_render` GPU suites live on RTX 3050 (Vulkan): tests/smoke.rs 2/2 +
  tests/golden.rs 8/8 (pass-order log == sequence; premul OVER vs CPU ref;
  additive ties both draw; alpha-test discard incl. normal validity;
  zero-alpha-no-color; higher-layer-wins; HDR pre/post tone map incl. bloom
  doubling; sRGB-vs-manual exactly-once; velocity math + history
  bookkeeping incl. cut serial + resize invalidation).
- `phase2_demo` re-run → PASS (no Phase 2 regression from the 4 fixes).
- Temporary `tests/probe_post.rs` deleted; no bisect/trace hacks remain
  (dispatch verified opaque → ordered_2d → transparent once each).

## Consequences

- Any change to the chain order, tone curve, threshold defaults, alpha table
  or MAIN_PASS_SEQUENCE is a VERSION event: goldens + smoke fail BY DESIGN.
- MVP limits recorded here (bloom-lite doubling, cross-pass layer override,
  2D = painter's algorithm with depth ties) are the declared WP07+ surface.
- The probe-derived chain model is the G5 contract: new goldens must compute
  references from `transfer(clamp(tone_map(chain)))`, not from staged colors.
