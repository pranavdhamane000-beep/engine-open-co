//! GPU smoke test (first hardware contact for the WP06 GPU half).
//!
//! Validates that ALL five WGSL shaders compile, every pipeline passes
//! validation, the pinned §15.6 target set allocates, and one frame of
//! the pinned MAIN_PASS_SEQUENCE executes end-to-end on real hardware.
//! Deep pixel goldens belong to tests/golden.rs (chunk 3).

use anim_render::{create_renderer, graph::MAIN_PASS_SEQUENCE};
use glam::Mat4;
use wgpu::Backends;

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
        label: Some("anim_render.smoke"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        memory_hints: Default::default(),
        trace: Default::default(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
    }))
    .ok()
}

/// One straight-authored ordered-2D quad + one premultiplied transparent
/// quad + one additive particle-band quad: covers all three blended
/// pipelines and every pass in the main sequence.
fn smoke_frame(aspect: f64) -> anim_render::RenderFrame {
    use anim_render::draw_list::{DrawItem, RenderFrame};
    use anim_render::graph::{
        MATERIAL_ORDERED_2D, MATERIAL_TRANSPARENT, PARTICLE_LAYER_ORDER,
    };
    use anim_render::{AlphaMode, CameraMatrices, RenderQuality};

    let camera = CameraMatrices::eval2d(None, aspect).expect("default camera");
    let jump = Mat4::IDENTITY;
    let items = vec![
        DrawItem {
            entity: anim_core::NodeId::new_v4(),
            draw_index: 0,
            layer_order: 0,
            material_pass: MATERIAL_ORDERED_2D,
            view_depth: 0.0,
            current_transform: jump,
            previous_transform: jump,
            alpha_mode: AlphaMode::Straight,
            opacity: 1.0,
            tint: [1.0, 0.0, 0.0, 1.0],
            velocity_valid: false,
        },
        DrawItem {
            entity: anim_core::NodeId::new_v4(),
            draw_index: 0,
            layer_order: 1,
            material_pass: MATERIAL_TRANSPARENT,
            view_depth: 0.0,
            current_transform: jump,
            previous_transform: jump,
            alpha_mode: AlphaMode::Premultiplied,
            opacity: 1.0,
            tint: [0.5, 1.0, 0.5, 0.5],
            velocity_valid: false,
        },
        DrawItem {
            entity: anim_core::NodeId::new_v4(),
            draw_index: 0,
            layer_order: PARTICLE_LAYER_ORDER,
            material_pass: MATERIAL_TRANSPARENT,
            view_depth: 0.0,
            current_transform: jump,
            previous_transform: jump,
            alpha_mode: AlphaMode::AdditivePremultiplied,
            opacity: 1.0,
            tint: [0.25, 0.25, 0.5, 1.0],
            velocity_valid: false,
        },
    ];
    RenderFrame {
        revision: anim_core::Revision::FIRST,
        tick: anim_sim::SimTick(0),
        items,
        camera,
        camera_id: None,
        cut: true, // first frame: no valid previous → cut semantics
        quality: RenderQuality::Preview,
    }
}

#[test]
fn full_main_sequence_runs_on_real_gpu() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available; skipping smoke");
        return;
    };

    let config = anim_render::RenderConfig {
        extent: wgpu::Extent3d { width: 64, height: 48, depth_or_array_layers: 1 },
        quality: anim_render::RenderQuality::Preview,
        // sRGB-aware target: the transfer must stay LINEAR (hardware
        // encodes exactly once — G5 contract exercised here).
        output_format: wgpu::TextureFormat::Rgba8UnormSrgb,
    };
    let mut renderer = create_renderer(&device, &queue, &config)
        .expect("renderer creation must pass format probes");

    let frame = smoke_frame(64.0 / 48.0);
    let scene =
        renderer.render_scene(&device, &queue, &frame).expect("frame render must succeed");

    // G5 command-log: executed mains == pinned MAIN_PASS_SEQUENCE.
    assert_eq!(
        scene.main_pass_log,
        MAIN_PASS_SEQUENCE.to_vec(),
        "executed pass log must match the pinned sequence exactly"
    );
    // Default post chain plan executed.
    assert!(scene.post_log.starts_with(&["post.motion_blur", "post.base_copy"]));
    assert!(scene.post_log.contains(&"post.tone_map"));
    assert_eq!(
        scene.post_log.iter().filter(|p| **p == "post.tone_map").count(),
        1,
        "tone map must run exactly once"
    );

    // Readback: engine byte counts (padded download correctness).
    let padded = scene.readback_output(&device, &queue).expect("output readback");
    let per_row = anim_render::readback::padded_bytes_per_row(4, 64);
    assert_eq!(padded.len(), (per_row * 48) as usize);

    let tight = scene.readback_output_unpadded(&device, &queue).expect("unpadded readback");
    assert_eq!(tight.len(), 64 * 48 * 4);

    // Resize flow: valid resize recreates and bumps generation; a zero
    // extent is REJECTED keeping targets intact (typed error).
    let gen_before = renderer.targets_generation();
    renderer
        .resize(&device, wgpu::Extent3d { width: 96, height: 96, depth_or_array_layers: 1 })
        .expect("valid resize");
    assert_eq!(renderer.targets_generation(), gen_before + 1);
    assert!(matches!(
        renderer.resize(
            &device,
            wgpu::Extent3d { width: 0, height: 96, depth_or_array_layers: 1 }
        ),
        Err(anim_render::RenderError::ZeroExtent(0, 96, 1))
    ));
    assert_eq!(
        renderer.targets_generation(),
        gen_before + 1,
        "bad resize must not bump generation"
    );

    // Second frame after resize (fresh history anyway after invalidation).
    let frame2 = smoke_frame(96.0 / 96.0);
    let scene2 = renderer
        .render_scene(&device, &queue, &frame2)
        .expect("post-resize frame render must succeed");
    assert_eq!(
        scene2.main_pass_log,
        MAIN_PASS_SEQUENCE.to_vec(),
        "post-resize frame must execute the identical pinned sequence"
    );
}

#[test]
fn device_reinit_preserves_nothing_but_revalidates_formats() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available; skipping reinit smoke");
        return;
    };
    let config = anim_render::RenderConfig {
        extent: wgpu::Extent3d { width: 8, height: 8, depth_or_array_layers: 1 },
        quality: anim_render::RenderQuality::Export,
        output_format: wgpu::TextureFormat::Rgba8Unorm, // manual transfer path
    };
    let renderer = create_renderer(&device, &queue, &config).expect("reinit");
    assert_eq!(renderer.output_format(), wgpu::TextureFormat::Rgba8Unorm);
    assert_eq!(renderer.extent().width, 8);
}
