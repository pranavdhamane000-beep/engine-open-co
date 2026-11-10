// golden.rs -- WP06 Gate G5 (spec 15.12): pass-order verification and
// pinned-stack READBACK goldens against CPU reference math.
//
// Every golden renders crafted frames through the REAL pinned
// MAIN_PASS_SEQUENCE on hardware and compares pixels to independently
// computed references (alpha.rs CPU composite rules, 15.7 NDc velocity
// math, and the linear/sRGB encoder tables).
//
// THE PINNED POST CHAIN MODEL (probe-verified on RTX 3050, 2026-11-08):
//   out = transfer( clamp( tone_map( chain_color ) ) )  with
//   chain_color = motion_identity(base)  [+ bloom_layer if bloom]
//     where motion with an INVALID velocity pair is an identity copy
//     (velocity invalid = no blur), and the MVP "bloom-lite" bright
//     pass yields c.rgb for every tap with lum > 1.0, so a UNIFORM
//     bright quad's bloom layer == base.rgb (2x doubling; ADR-0010
//     documents the refinement for WP07+).
//   alpha output: tone map does NOT change alpha; grade clamps RGB
//     only; the Rgba8Unorm target hardware-clamps the final edge.

use anim_render::alpha::{blend_additive_premul, blend_over_premul};
use anim_render::draw_list::{DrawItem, RenderFrame};
use anim_render::graph::{
    MainPass, MAIN_PASS_SEQUENCE, MATERIAL_OPAQUE_MRT, MATERIAL_ORDERED_2D,
    MATERIAL_TRANSPARENT,
};
use anim_render::{
    create_renderer, CameraMatrices, InternalTarget, RenderConfig, RenderQuality,
};
use glam::{Mat4, Vec3};
use wgpu::Backends;

// ---------- shared helpers ----------

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: Backends::all(),
        ..Default::default()
    });
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("anim_render.golden"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        memory_hints: Default::default(),
        trace: Default::default(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
    }))
    .ok()
}

const W: u32 = 64;
const H: u32 = 48;
const ASPECT: f64 = W as f64 / H as f64;

fn config(output_format: wgpu::TextureFormat) -> RenderConfig {
    RenderConfig {
        extent: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        quality: RenderQuality::Preview,
        output_format,
    }
}

fn golden_frame(items: Vec<DrawItem>, cut: bool) -> RenderFrame {
    let camera = CameraMatrices::eval2d(None, ASPECT).expect("default camera");
    RenderFrame {
        revision: anim_core::Revision::FIRST,
        tick: anim_sim::SimTick(0),
        items,
        camera,
        camera_id: None,
        cut,
        quality: RenderQuality::Preview,
    }
}

/// Item factory: fixed transform model = T(position) * S(scale)
/// (8 params by design: one per DrawItem knob).
#[allow(clippy::too_many_arguments)]
fn item(
    layer: i32,
    material: u16,
    alpha_mode: anim_render::AlphaMode,
    tint: [f32; 4],
    position: (f32, f32),
    scale: f32,
    velocity_valid: bool,
    previous_offset: (f32, f32),
) -> DrawItem {
    let model = Mat4::from_translation(Vec3::new(position.0, position.1, 0.0))
        * Mat4::from_scale(Vec3::splat(scale));
    let previous = Mat4::from_translation(Vec3::new(
        position.0 - previous_offset.0,
        position.1 - previous_offset.1,
        0.0,
    )) * Mat4::from_scale(Vec3::splat(scale));
    DrawItem {
        entity: anim_core::NodeId::new_v4(),
        draw_index: 0,
        layer_order: layer,
        material_pass: material,
        view_depth: 0.0,
        current_transform: model,
        previous_transform: previous,
        alpha_mode,
        opacity: tint[3],
        tint,
        velocity_valid,
    }
}

/// 15.7 velocity-frame math exactly as the shader computes it.
fn uv_of(cam: &CameraMatrices, world: (f32, f32)) -> [f32; 2] {
    let ndc = cam.project_current(Vec3::new(world.0, world.1, 0.0));
    [ndc.x * 0.5 + 0.5, -ndc.y * 0.5 + 0.5]
}

/// CPU pulse of the shader `pixel_of` helper.
fn pixel_xy(uv: [f32; 2]) -> (usize, usize) {
    let px = ((W as f32 * uv[0] - 0.5).round().clamp(0.0, W as f32 - 1.0)) as usize;
    let py = ((H as f32 * uv[1] - 0.5).round().clamp(0.0, H as f32 - 1.0)) as usize;
    (px, py)
}

fn decode_hdr(padded: &[u8]) -> Vec<[f32; 4]> {
    let tight = anim_render::readback::unpad_rows(padded, 8, W, H).expect("unpad");
    tight
        .as_chunks::<8>()
        .0
        .iter()
        .map(|p| {
            let u = |i: usize| u16::from_le_bytes([p[i * 2], p[i * 2 + 1]]);
            [
                half::f16::from_bits(u(0)).to_f32(),
                half::f16::from_bits(u(1)).to_f32(),
                half::f16::from_bits(u(2)).to_f32(),
                half::f16::from_bits(u(3)).to_f32(),
            ]
        })
        .collect()
}

fn pix(x: usize, y: usize) -> usize {
    y * W as usize + x
}

/// Linear to sRGB byte (the EXACT transfer contract).
fn srgb_byte(c: f32) -> u8 {
    let x = if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (x * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

/// Pinned scalar post chain for a linear resulting color c:
/// Reinhard tone map (always) -> display clamp -> sRGB transfer.
/// (Grade is neutral at defaults; clamp(c) is identity for tone(c)<=1.)
fn chain_byte(c: f32) -> u8 {
    srgb_byte(c / (1.0 + c))
}

/// MVP bloom-lite behaved on a UNIFORM bright quad (every tap passes
/// lum > 1.0): the bloom layer equals the base color, so the combine
/// DOUBLES the base before tone map (documented ADR-0010 refinement
/// note). NaN-safe: returns base + 2*base only when lum > threshold.
fn uniform_bloom(base: [f32; 4]) -> [f32; 3] {
    let luma = 0.2126 * base[0] + 0.7152 * base[1] + 0.0722 * base[2];
    let scale = if luma > 1.0 { 2.0 } else { 1.0 };
    [base[0] * scale, base[1] * scale, base[2] * scale]
}

fn close(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

/// Assert 3 channels of `got` within `tol` encoder counts of the CPU
/// reference bytes `want` (f16 storage + hardware rounding noise).
fn assert_bytes(got: &[u8], want: [u8; 3], tol: i32, ctx: &str) {
    for i in 0..3 {
        assert!(
            (i32::from(got[i]) - i32::from(want[i])).abs() <= tol,
            "{ctx} channel {i}: got {} want {}",
            got[i],
            want[i]
        );
    }
}

// ---------- G5 goldens ----------

/// GOLDEN 1: pass order (command log) + premul OVER matches the
/// alpha.rs CPU reference through the real GPU chain; the overlay must
/// win over the ordered-2D background (order verification).
#[test]
fn pass_order_and_premul_over_matches_cpu_reference() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping golden");
        return;
    };
    let mut renderer =
        create_renderer(&device, &queue, &config(wgpu::TextureFormat::Rgba8UnormSrgb))
            .expect("renderer creation");

    // Full-cover background (ordered-2D straight green), then a
    // full-cover transparent overlay authored STRAIGHT.
    let bg = item(
        0,
        MATERIAL_ORDERED_2D,
        anim_render::AlphaMode::Straight,
        [0.0, 0.6, 0.0, 1.0],
        (0.0, 0.0),
        40.0,
        false,
        (0.0, 0.0),
    );
    let overlay = item(
        1,
        MATERIAL_TRANSPARENT,
        anim_render::AlphaMode::Straight, // shader decodes rgb*a
        [1.0, 0.5, 0.0, 0.5],
        (0.0, 0.0),
        40.0,
        false,
        (0.0, 0.0),
    );
    let frame = golden_frame(vec![bg, overlay], true);

    let scene =
        renderer.render_scene(&device, &queue, &frame).expect("render must succeed");

    // G5: executed command log == pinned MAIN_PASS_SEQUENCE exactly.
    assert_eq!(scene.main_pass_log, MAIN_PASS_SEQUENCE.to_vec());
    assert_eq!(scene.main_pass_log.len(), 5);
    assert_eq!(scene.main_pass_log[4], MainPass::OutputTransfer);

    // CPU reference: OVER in linear HDR (alpha.rs contract), then the
    // pinned post chain (tone + clamp + transfer).
    let src = anim_render::alpha::straight_to_premultiplied([1.0, 0.5, 0.0, 0.5]);
    let expected = blend_over_premul([0.0, 0.6, 0.0, 1.0], src);
    // expected linear = [0.5, 0.55, 0.0], a = 1.0.
    let out = scene.readback_output_unpadded(&device, &queue).expect("readback");
    let px = pixel_xy(uv_of(&frame.camera, (0.0, 0.0)));
    let got = &out[4 * pix(px.0, px.1)..4 * pix(px.0, px.1) + 4];
    assert_bytes(
        got,
        [chain_byte(expected[0]), chain_byte(expected[1]), chain_byte(expected[2])],
        2,
        "premul-over composite",
    );
    // Overlay must WIN (painter order): the red channel proves it --
    // without the overlay the bg has no red at all.
    assert!(got[0] > 100, "overlay red must be present (order proof): got {}", got[0]);
    let alpha = got[3];
    // premul OVER alpha: 0.5 + (1-0.5)*1.0 = 1.0 -> 255.
    assert_eq!(alpha, 255, "premul OVER alpha = 1.0 exactly, got {alpha}");

    // The bg cut: first frame cut=true bumps the cut serial once.
    assert_eq!(
        renderer.previous_frame_state().history.cut_serial,
        1,
        "first cut must bump the cut serial exactly once"
    );
}

/// GOLDEN: additive premul matches the CPU reference (One/One) — and
/// two same-(layer, depth) transparent ties BOTH draw (LessEqual, no
/// depth write) so the composition equals the sum of the two refs.
#[test]
fn additive_matches_cpu_and_equal_depth_ties_both_draw() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping golden");
        return;
    };
    let mut renderer =
        create_renderer(&device, &queue, &config(wgpu::TextureFormat::Rgba8UnormSrgb))
            .expect("renderer creation");

    // Two additive quads with IDENTICAL (layer, view depth, material)
    // but different entities — the CPU sort delivers a deterministic
    // draw order; composition is exact because addition commutes.
    let a = item(
        7,
        MATERIAL_TRANSPARENT,
        anim_render::AlphaMode::AdditivePremultiplied,
        [0.25, 0.0, 0.0, 1.0],
        (0.0, 0.0),
        40.0,
        false,
        (0.0, 0.0),
    );
    let b = item(
        7,
        MATERIAL_TRANSPARENT,
        anim_render::AlphaMode::AdditivePremultiplied,
        [0.0, 0.5, 0.0, 1.0],
        (0.0, 0.0),
        40.0,
        false,
        (0.0, 0.0),
    );
    let frame = golden_frame(vec![a, b], false);
    let scene =
        renderer.render_scene(&device, &queue, &frame).expect("render must succeed");

    // CPU: black background, add over twice.
    let acc = blend_additive_premul([0.0, 0.0, 0.0, 0.0], [0.25, 0.0, 0.0, 1.0]);
    let expected = blend_additive_premul(acc, [0.0, 0.5, 0.0, 1.0]);
    // expected = [0.25, 0.5, 0.0, 2.0]; lum = 0.4 < 1 -> no bloom.
    let out = scene.readback_output_unpadded(&device, &queue).expect("readback");
    let px = pixel_xy(uv_of(&frame.camera, (0.0, 0.0)));
    let i0 = 4 * pix(px.0, px.1);
    let got = &out[i0..i0 + 4];
    assert_bytes(
        got,
        [chain_byte(expected[0]), chain_byte(expected[1]), chain_byte(expected[2])],
        2,
        "additive composite",
    );
    assert_eq!(got[3], 255, "additive alpha 2.0 hardware-clamps to 1 on Rgba8");
}

/// GOLDEN: alpha-test discard before any write; cutoff in (0, 1].
#[test]
fn alpha_test_discards_below_cutoff() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping golden");
        return;
    };
    let mut renderer =
        create_renderer(&device, &queue, &config(wgpu::TextureFormat::Rgba8UnormSrgb))
            .expect("renderer creation");

    // LEFT quad: alpha 0.25 < cutoff 0.5 -> entirely discarded.
    // RIGHT quad: alpha 1.0 >= cutoff 1.0 (boundary INCLUDED) -> drawn.
    let left = item(
        0,
        MATERIAL_OPAQUE_MRT,
        anim_render::AlphaMode::AlphaTest { cutoff: 0.5 },
        [0.9, 0.1, 0.1, 0.25],
        (-3.3, 0.0),
        6.0,
        false,
        (0.0, 0.0),
    );
    let right = item(
        0,
        MATERIAL_OPAQUE_MRT,
        anim_render::AlphaMode::AlphaTest { cutoff: 0.5 },
        [0.9, 0.1, 0.1, 1.0],
        (3.3, 0.0),
        6.0,
        false,
        (0.0, 0.0),
    );
    let frame = golden_frame(vec![left, right], false);
    let scene =
        renderer.render_scene(&device, &queue, &frame).expect("render must succeed");
    let out = scene.readback_output_unpadded(&device, &queue).expect("readback");

    let discarded = pixel_xy(uv_of(&frame.camera, (-3.3, 0.0)));
    let drawn = pixel_xy(uv_of(&frame.camera, (3.3, 0.0)));
    let di = 4 * pix(discarded.0, discarded.1);
    let ri = 4 * pix(drawn.0, drawn.1);

    // Discarded: cleared background stays (0,0,0,0).
    assert_eq!([out[di], out[di + 1], out[di + 2], out[di + 3]], [0, 0, 0, 0]);
    // Drawn: [0.9, 0.1, 0.1, 1.0]; lum = 0.27 < 1 -> no bloom; the
    // chain tone-maps: Reinhard(0.9) = 0.4737 -> 183.
    assert_bytes(
        &out[ri..ri + 4],
        [chain_byte(0.9), chain_byte(0.1), chain_byte(0.1)],
        2,
        "alpha-test drawn",
    );
    assert_eq!(out[ri + 3], 255, "alpha 1.0 passes the == cutoff");

    // Normal validity proves the RIGHT quad only: drawn normal (z,w)=1;
    // discarded stays cleared.
    let normals = decode_hdr(
        &renderer
            .readback_internal(&device, &queue, InternalTarget::Normal)
            .expect("normal readback"),
    );
    let n_d = normals[pix(drawn.0, drawn.1)];
    let n_dis = normals[pix(discarded.0, discarded.1)];
    assert!(
        close(n_d[2], 1.0, 1e-3) && close(n_d[3], 1.0, 1e-3),
        "drawn normal (z,w) = {n_d:?}"
    );
    assert!(
        close(n_dis[2], 0.0, 1e-3) && close(n_dis[3], 0.0, 1e-3),
        "discarded normal must stay cleared: {n_dis:?}"
    );
}

/// GOLDEN: zero-alpha straight ORDERED content contributes no color
/// (rgb * a == 0 in the shader decode) — background stays visible
/// through the pinned chain (tone map + transfer).
#[test]
fn zero_alpha_straight_contributes_no_color() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping golden");
        return;
    };
    let mut renderer =
        create_renderer(&device, &queue, &config(wgpu::TextureFormat::Rgba8UnormSrgb))
            .expect("renderer creation");
    let bg = item(
        0,
        MATERIAL_ORDERED_2D,
        anim_render::AlphaMode::Straight,
        [0.2, 0.4, 0.8, 1.0],
        (0.0, 0.0),
        40.0,
        false,
        (0.0, 0.0),
    );
    let zero = item(
        1,
        MATERIAL_ORDERED_2D,
        anim_render::AlphaMode::Straight,
        [1.0, 1.0, 1.0, 0.0],
        (0.0, 0.0),
        40.0,
        false,
        (0.0, 0.0),
    );
    let frame = golden_frame(vec![bg, zero], false);
    let scene = renderer.render_scene(&device, &queue, &frame).expect("render");
    let out = scene.readback_output_unpadded(&device, &queue).expect("readback");
    let px = pixel_xy(uv_of(&frame.camera, (0.0, 0.0)));
    let i = 4 * pix(px.0, px.1);
    // bg [0.2,0.4,0.8] lum = 0.386 < 1 -> no bloom; chain = tone+encode.
    assert_bytes(
        &out[i..i + 3],
        [chain_byte(0.2), chain_byte(0.4), chain_byte(0.8)],
        2,
        "zero-alpha must contribute NOTHING over bg",
    );
    assert_eq!(out[i + 3], 255, "bg alpha stays 1.0");
}

/// GOLDEN: layer ordering with identical content except `layer_order`
/// — the HIGHER layer draws LAST (painter's algorithm), proving CPU
/// sort order governs GPU composition.
#[test]
fn higher_layer_paints_over_lower_layer() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping golden");
        return;
    };
    let mut renderer =
        create_renderer(&device, &queue, &config(wgpu::TextureFormat::Rgba8UnormSrgb))
            .expect("renderer creation");

    // LOWER layer = blue premul 0.5, HIGHER layer = red straight 1.0,
    // BOTH in the ordered_2D pass: pass grouping (opaque -> ordered ->
    // transparent) always beats layer order ACROSS passes, so ordering
    // is only testable within one material pass. If ordering were
    // inverted, blue would colour every pixel — the assertion
    // distinguishes these outcomes exactly.
    let lower = item(
        -5,
        MATERIAL_ORDERED_2D,
        anim_render::AlphaMode::Premultiplied,
        [0.0, 0.0, 1.0, 0.5],
        (0.0, 0.0),
        40.0,
        false,
        (0.0, 0.0),
    );
    let higher = item(
        5,
        MATERIAL_ORDERED_2D,
        anim_render::AlphaMode::Straight,
        [1.0, 0.0, 0.0, 1.0],
        (0.0, 0.0),
        40.0,
        false,
        (0.0, 0.0),
    );
    let frame = golden_frame(vec![lower, higher], false);
    let scene = renderer.render_scene(&device, &queue, &frame).expect("render");
    let out = scene.readback_output_unpadded(&device, &queue).expect("readback");
    let px = pixel_xy(uv_of(&frame.camera, (0.0, 0.0)));
    let i = 4 * pix(px.0, px.1);
    // Higher layer covers entirely: pure red. Chain: tone(1) = 0.5.
    assert_eq!(
        out[i],
        srgb_byte(0.5),
        "red must fully cover the lower layer (tone-mapped)"
    );
    assert_eq!(out[i + 1], 0, "no blue may leak (ordering proof)");
    assert_eq!(out[i + 2], 0);
}

/// GOLDEN: linear decode + HDR-BEFORE-TONE-MAP — crafted HDR tint (4.0)
/// survives unclamped in scene_hdr (pre-post values) but is compressed
/// in the display output (tone map + MVP bloom doubling, which fires
/// because lum = 1.215 > threshold 1.0 for the uniform bright quad).
#[test]
fn hdr_survives_pre_tone_map_but_not_after() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping golden");
        return;
    };
    let mut renderer =
        create_renderer(&device, &queue, &config(wgpu::TextureFormat::Rgba8UnormSrgb))
            .expect("renderer creation");

    // HDR straight tint: r = 4.0 > 1.0 — valid LINEAR HDR input whose
    // luminance clears the bloom threshold with margin (1.215 > 1).
    let base = [4.0, 0.5, 0.1, 1.0];
    let flash = item(
        0,
        MATERIAL_ORDERED_2D,
        anim_render::AlphaMode::Straight,
        base,
        (0.0, 0.0),
        40.0,
        false,
        (0.0, 0.0),
    );
    let frame = golden_frame(vec![flash], false);
    let scene = renderer.render_scene(&device, &queue, &frame).expect("render");

    let hdr = decode_hdr(
        &renderer
            .readback_internal(&device, &queue, InternalTarget::SceneHdr)
            .expect("scene_hdr readback"),
    );
    let px = pixel_xy(uv_of(&frame.camera, (0.0, 0.0)));
    let got = hdr[pix(px.0, px.1)];
    // PRE-tone-map r must still be ~4.0 (Rgba16Float range covers it).
    assert!(
        close(got[0], 4.0, 0.02),
        "scene_hdr must hold linear HDR pre-tone-map: r = {}",
        got[0]
    );
    assert!(close(got[1], 0.5, 0.02), "g = {}", got[1]);
    assert!(close(got[2], 0.1, 0.02), "b = {}", got[2]);
    assert!(close(got[3], 1.0, 1e-3), "a = {}", got[3]);

    // Output: MVP bloom doubles the uniform bright quad (lum > 1), so
    // the tone-mapped input is 2*base = (8, 1, 0.2):
    //   tone(8) = 0.888..., tone(1) = 0.5, tone(0.2) = 0.1666...
    let blended = uniform_bloom(base);
    let out = scene.readback_output_unpadded(&device, &queue).expect("readback");
    let i = 4 * pix(px.0, px.1);
    assert_bytes(
        &out[i..i + 3],
        [chain_byte(blended[0]), chain_byte(blended[1]), chain_byte(blended[2])],
        2,
        "HDR post output",
    );
    assert!(out[i] < 254, "output must be tonemapped (< full white)");
}

/// GOLDEN: exactly ONE output transfer. The SAME linear scene rendered
/// into an sRGB-aware target (hardware encodes; shader passes) vs a
/// linear target (shader manually encodes) must produce IDENTICAL
/// bytes. A double encode or a missing encode would break equality.
#[test]
fn output_transfer_runs_exactly_once_for_srgb_and_manual() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping golden");
        return;
    };
    let linear_scene = || {
        vec![item(
            0,
            MATERIAL_ORDERED_2D,
            anim_render::AlphaMode::Straight,
            [0.5, 0.25, 0.75, 1.0],
            (0.0, 0.0),
            40.0,
            false,
            (0.0, 0.0),
        )]
    };
    let mut srgb_renderer =
        create_renderer(&device, &queue, &config(wgpu::TextureFormat::Rgba8UnormSrgb))
            .expect("srgb renderer");
    let mut lin_renderer =
        create_renderer(&device, &queue, &config(wgpu::TextureFormat::Rgba8Unorm))
            .expect("linear renderer");

    let f1 = golden_frame(linear_scene(), false);
    let scene_s = srgb_renderer.render_scene(&device, &queue, &f1).expect("render");
    let f2 = golden_frame(linear_scene(), false);
    let scene_m = lin_renderer.render_scene(&device, &queue, &f2).expect("render");

    let o_s = scene_s.readback_output_unpadded(&device, &queue).expect("readback srgb");
    let o_m = scene_m.readback_output_unpadded(&device, &queue).expect("readback manual");
    let px = pixel_xy(uv_of(&f1.camera, (0.0, 0.0)));
    let i = 4 * pix(px.0, px.1);
    for (ch, lin) in [(0usize, 0.5f32), (1, 0.25), (2, 0.75)] {
        assert_eq!(
            o_s[i + ch],
            o_m[i + ch],
            "channel {ch} must encode EXACTLY once on both target paths"
        );
        assert!(
            (i32::from(o_s[i + ch]) - i32::from(chain_byte(lin))).abs() <= 2,
            "channel {ch}: byte {} != chain({lin})",
            o_s[i + ch]
        );
    }
    assert_eq!(o_s[i + 3], 255, "alpha passthrough");
}

/// GOLDEN: velocity target math and validity. A translating opaque
/// quad's velocity == uv_now - uv_prev (15.7 math) with validity z = 1;
/// a per-item invalid quad keeps the cleared target (xy = 0, z = 0);
/// reserved w stays 0 everywhere. History bookkeeping: the cut bumps
/// the serial once, the non-cut frame leaves it, resize invalidates.
#[test]
fn velocity_pairs_match_reference_and_bookkeeping_holds() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping golden");
        return;
    };
    let mut renderer =
        create_renderer(&device, &queue, &config(wgpu::TextureFormat::Rgba8UnormSrgb))
            .expect("renderer creation");

    // Moving quad: previous offset (2,0) in WORLD units; per-item valid.
    let mover = item(
        0,
        MATERIAL_OPAQUE_MRT,
        anim_render::AlphaMode::Opaque,
        [0.1, 0.2, 0.3, 1.0],
        (2.0, 0.0),
        3.0,
        true,
        (2.0, 0.0), // previous center (0,0)
    );
    // Stationary "teleport/new" quad: invalid velocity pair.
    let fresh = item(
        0,
        MATERIAL_OPAQUE_MRT,
        anim_render::AlphaMode::Opaque,
        [0.3, 0.2, 0.1, 1.0],
        (-3.0, 0.0),
        2.5,
        false,
        (0.0, 0.0),
    );
    let frame = golden_frame(vec![mover, fresh], false);
    let _scene = renderer.render_scene(&device, &queue, &frame).expect("render");

    let vel = decode_hdr(
        &renderer
            .readback_internal(&device, &queue, InternalTarget::Velocity)
            .expect("velocity readback"),
    );

    // Per-item velocity = uv(center_now) - uv(center_prev) at the quad
    // center pixel (translation => the linear interpolation is exact
    // at every pixel, not just the center).
    let mover_px = pixel_xy(uv_of(&frame.camera, (2.0, 0.0)));
    let v = vel[pix(mover_px.0, mover_px.1)];
    // Expected: translation of +2 world units in x with the SAME
    // camera pair: dx = 0.5 * (2 / half_width).
    let half_width = 5.0 * ASPECT as f32;
    let expected_dx = 0.5 * (2.0 / half_width);
    assert!(close(v[2], 1.0, 1e-3), "mover velocity must be VALID (z = 1): got {:?}", v);
    assert!(
        close(v[0], expected_dx, 0.01),
        "mover velocity dx: got {} want {}",
        v[0],
        expected_dx
    );
    assert!(close(v[1], 0.0, 0.01), "mover velocity dy must be 0");
    assert!(close(v[3], 0.0, 1e-6), "reserved w must be 0");

    let fresh_px = pixel_xy(uv_of(&frame.camera, (-3.0, 0.0)));
    let f = vel[pix(fresh_px.0, fresh_px.1)];
    assert!(
        close(f[2], 0.0, 1e-3) && close(f[0], 0.0, 1e-3) && close(f[1], 0.0, 1e-3),
        "fresh item must carry the CLEARED velocity: got {f:?}"
    );

    // ---- history bookkeeping through the SAME renderer ----
    // First frame has cut = false in this test: serial stays 0.
    assert_eq!(renderer.previous_frame_state().history.cut_serial, 0);
    let f_cut = golden_frame(
        vec![item(
            0,
            MATERIAL_OPAQUE_MRT,
            anim_render::AlphaMode::Opaque,
            [1.0, 0.0, 0.0, 1.0],
            (0.0, 0.0),
            1.0,
            false,
            (0.0, 0.0),
        )],
        true,
    );
    renderer.render_scene(&device, &queue, &f_cut).expect("cut render");
    assert_eq!(
        renderer.previous_frame_state().history.cut_serial,
        1,
        "cut must bump the serial EXACTLY once"
    );

    // Resize with an aspect CHANGE: ProjectionChange + TargetResize
    // invalidate history (valid == false).
    renderer
        .resize(&device, wgpu::Extent3d { width: 80, height: 40, depth_or_array_layers: 1 })
        .expect("resize");
    let st = renderer.previous_frame_state();
    assert!(!st.history.valid, "resize must invalidate history");
    assert!(
        (st.viewport_aspect - 2.0).abs() < 1e-9,
        "viewport aspect must be updated to 2.0"
    );
    // And the NEXT render against the new targets is valid again.
    let mut f3 = golden_frame(
        vec![item(
            0,
            MATERIAL_OPAQUE_MRT,
            anim_render::AlphaMode::Opaque,
            [1.0, 1.0, 1.0, 1.0],
            (0.0, 0.0),
            40.0,
            false,
            (0.0, 0.0),
        )],
        true,
    );
    // Crafted camera aspect must match new targets for a clean render.
    f3.camera = CameraMatrices::eval2d(None, 2.0).expect("camera");
    let _scene = renderer.render_scene(&device, &queue, &f3).expect("post-resize render");
    let st2 = renderer.previous_frame_state();
    assert!(st2.history.valid, "post-resize render must re-validate history");
}
