//! Live GPU budget report (spec §16.12 G6 anchor: anim_headless
//! budget_report). Runs on the real adapter, correlates by LUID,
//! prints budget/usage/eviction/pressure diagnostics every frame.

use anim_render::{
    create_renderer, memory::dxgi::DxgiTelemetry, IdentityHints, IdentityState,
    RenderConfig,
};
use pollster::FutureExt;
use std::time::Duration;
use wgpu::Backends;

fn main() {
    println!("=== anim_headless budget_report (WP07 chunk 2) ===");

    // 1. Get a real device (caller chooses the adapter — D-11).
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: Backends::all(),
        ..Default::default()
    });
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        })
        .block_on()
        .expect("no adapter");
    println!("Adapter: {}", adapter.get_info().name);

    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("anim_headless.budget_report"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: Default::default(),
            trace: Default::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
        })
        .block_on()
        .expect("device request failed");

    // 2. Build renderer + memory manager.
    let config = RenderConfig {
        extent: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 },
        quality: anim_render::RenderQuality::Preview,
        output_format: wgpu::TextureFormat::Rgba8UnormSrgb,
    };
    let mut renderer =
        create_renderer(&device, &queue, &config).expect("renderer creation failed");
    let _ = &queue; // suppress unused warning

    // 3. Correlate DXGI telemetry (ADR-0003: LUID from the Vulkan path;
    //    here we fall back to vendor/device/description if no LUID).
    //    wgpu Device doesn't expose adapter(); use the Instance adapter we already have.
    let adapter_info = adapter.get_info();
    let hints = IdentityHints {
        luid: None, // caller fills if available (Vulkan deviceLUID)
        vendor_id: Some(adapter_info.vendor),
        device_id: Some(adapter_info.device),
        description: Some(adapter_info.name),
    };
    println!("Correlation hints: {:?}", hints);
    if let Some(telemetry) = DxgiTelemetry::resolve(&hints).expect("DXGI resolve failed") {
        let id_state = telemetry.identity_state();
        // Extract AdapterIdentity from Resolved variant
        let adapter_identity = match &id_state {
            anim_render::memory::IdentityState::Resolved(id) => id,
            _ => unreachable!(),
        };
        println!(
            "DXGI correlated: {} (LUID={:x}, VRAM={} MiB)",
            adapter_identity.description,
            adapter_identity.luid,
            adapter_identity.dedicated_video_memory / (1024 * 1024)
        );
        renderer.memory_mut().set_telemetry(Box::new(telemetry));
        renderer.memory_mut().set_identity(id_state);
    } else {
        println!("DXGI correlation unavailable — conservative estimates only");
        renderer.memory_mut().set_identity(IdentityState::Unavailable {
            reason: "no confident LUID/vendor/device match".into(),
        });
    }

    // 4. Render loop with periodic budget reporting.
    let frame = smoke_frame(1.0);
    for i in 0..60 {
        let _scene =
            renderer.render_scene(&device, &queue, &frame).expect("frame render failed");

        // Sample pressure each frame (production would throttle by
        // query_interval; this demo forces it for visibility).
        let _ = renderer.memory_mut().sample_pressure().expect("pressure sample failed");

        // Print metrics every 15 frames.
        if i % 15 == 0 {
            let m = renderer.memory().metrics();
            println!("\n--- Frame {} ---", i);
            println!("  {}", m.summary_line());
            if let Some(s) = m.last_snapshot {
                println!(
                    "  budget={} MiB usage={} MiB effective={} MiB accounted={} MiB",
                    s.budget / (1024 * 1024),
                    s.current_usage / (1024 * 1024),
                    s.effective_usage / (1024 * 1024),
                    s.accounted_usage / (1024 * 1024),
                );
                println!(
                    "  soft_target={} MiB release_wm={} MiB",
                    (s.budget as f32 * 0.80) as u64 / (1024 * 1024),
                    ((s.budget as f32 * 0.80) as u64 - 64 * 1024 * 1024) / (1024 * 1024),
                );
            }
            println!(
                "  evictions={} evicted_bytes={} MiB stalls={} pressure_events={}",
                m.counters.evictions,
                m.counters.evicted_bytes / (1024 * 1024),
                m.counters.reload_stalls,
                m.counters.persistent_pressure_events,
            );
        }

        // Small sleep so the output is readable.
        std::thread::sleep(Duration::from_millis(33));
    }

    println!("\n=== budget_report completed ===");
}

/// Minimal smoke frame (same pattern as tests/smoke.rs).
fn smoke_frame(aspect: f64) -> anim_render::RenderFrame {
    use anim_render::draw_list::{DrawItem, RenderFrame};
    use anim_render::graph::{MATERIAL_ORDERED_2D, MATERIAL_TRANSPARENT};
    use anim_render::{AlphaMode, CameraMatrices, RenderQuality};
    use glam::Mat4;

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
    ];
    RenderFrame {
        revision: anim_core::Revision::FIRST,
        tick: anim_sim::SimTick(0),
        items,
        camera,
        camera_id: None,
        cut: true,
        quality: RenderQuality::Preview,
    }
}
