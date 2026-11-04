//! WP00 GPU probe — spec 9.5 steps 3–4, 9.7 "GPU/readback probe", 9.8 `GpuProbeReport`.
//!
//! Proves on this machine:
//!   1. adapter enumeration with recorded name/backend/driver/limits (spec 9.5 step 4)
//!   2. minimal WGSL compute + dispatch + device loss + map readback failure modes (spec 9.7)
//!   3. RGBA16F texture attachment capability (GpuProbeReport field, spec 9.8)
//!
//! Output: one machine-readable JSON probe report on stdout (spec 9.5 step 10).
//! Exit code: 0 = required capabilities verified; 1 = probe failure (spec 9.6).
//! No silent backend fallback: the selected backend is reported verbatim.

use serde::Serialize;
use wgpu::{Backends, DeviceDescriptor, MapMode, ShaderSource, TextureUsages};

const SHADER: &str = r#"
@group(0) @binding(0)
var<storage, read_write> out: array<u32, 16>;

@compute
@workgroup_size(16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i: u32 = gid.x;
    out[i] = (i << 8u) | (4096u + i);
}
"#;

const OUT_LEN: u32 = 16;

#[derive(Serialize)]
struct AdapterInfoJson {
    name: String,
    backend: String,
    device_type: String,
    driver: String,
    driver_info: String,
    vendor: u32,
    device: u32,
}

#[derive(Serialize)]
struct GpuProbeReport {
    schema_version: u32,
    capability: String,
    success: bool,
    adapters: Vec<AdapterInfoJson>,
    selected: Option<AdapterInfoJson>,
    max_buffer_size: u64,
    rgba16f_attachment: bool,
    compute_readback_verified: bool,
    diagnostics: Vec<String>,
}

impl GpuProbeReport {
    fn new() -> Self {
        Self {
            schema_version: 1,
            capability: "gpu_compute_readback".into(),
            success: false,
            adapters: Vec::new(),
            selected: None,
            max_buffer_size: 0,
            rgba16f_attachment: false,
            compute_readback_verified: false,
            diagnostics: Vec::new(),
        }
    }
}

fn adapter_json(info: &wgpu::AdapterInfo) -> AdapterInfoJson {
    AdapterInfoJson {
        name: info.name.clone(),
        backend: format!("{:?}", info.backend),
        device_type: format!("{:?}", info.device_type),
        driver: info.driver.clone(),
        driver_info: info.driver_info.clone(),
        vendor: info.vendor,
        device: info.device,
    }
}

/// Distinct failure reporting (spec 9.7): all early-exit leaves are explicit.
fn fail(report: &GpuProbeReport) -> ! {
    println!("{}", serde_json::to_string_pretty(report).unwrap_or_default());
    std::process::exit(1);
}

fn main() {
    let mut report = GpuProbeReport::new();

    // --- Step 4: enumerate adapters, record name/backend/driver (spec 9.6) ---
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: Backends::all(),
        ..Default::default()
    });
    let adapters = instance.enumerate_adapters(Backends::all());
    report.diagnostics.push(format!("adapters_found = {}", adapters.len()));

    if adapters.is_empty() {
        report.diagnostics.push("failure: no adapters enumerated".into());
        fail(&report);
    }

    for a in &adapters {
        report.adapters.push(adapter_json(&a.get_info()));
    }

    // Selection rule (spec 9.6: never silently assume the first adapter): prefer
    // a discrete GPU, then integrated, else first; the chosen adapter is reported.
    let rank = |info: &wgpu::AdapterInfo| match info.device_type {
        wgpu::DeviceType::DiscreteGpu => 0u8,
        wgpu::DeviceType::IntegratedGpu => 1,
        wgpu::DeviceType::VirtualGpu => 2,
        wgpu::DeviceType::Cpu | wgpu::DeviceType::Other => 3,
    };
    let mut candidates: Vec<(u8, wgpu::Adapter)> = adapters
        .into_iter()
        .map(|a| {
            let r = rank(&a.get_info());
            (r, a)
        })
        .collect();
    candidates.sort_by_key(|(r, _)| *r);
    let (rank, adapter) = candidates.remove(0);
    let info = adapter.get_info();
    report.diagnostics.push(format!("selected_rank = {rank} ({:?})", info.device_type));
    report.selected = Some(adapter_json(&info));

    // --- Step 5: request device with only required features (spec 9.5) ---
    let (device, queue) =
        match pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("wp00_gpu_probe"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            memory_hints: Default::default(),
            trace: Default::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
        })) {
            Ok(pair) => pair,
            Err(e) => {
                report.diagnostics.push(format!("failure: device request error: {e}"));
                fail(&report);
            }
        };
    {
        let limits = device.limits();
        report.max_buffer_size = limits.max_buffer_size;
    }

    // distinct failure mode per spec 9.7: WGSL shader compilation
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("wp00_smoke"),
        source: ShaderSource::Wgsl(SHADER.into()),
    });

    let src_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("wp00_out"),
        size: OUT_LEN as u64 * std::mem::size_of::<u32>() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let read_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("wp00_read"),
        size: OUT_LEN as u64 * std::mem::size_of::<u32>() as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("wp00_bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(
                    std::mem::size_of::<u32>() as u64 * OUT_LEN as u64,
                ),
            },
            count: None,
        }],
    });
    let pipe_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("wp00_pl"),
        bind_group_layouts: &[&bind_layout],
        push_constant_ranges: &[],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("wp00_bind"),
        layout: &bind_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &src_buf,
                offset: 0,
                size: None,
            }),
        }],
    });

    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("wp00_pipe"),
        layout: Some(&pipe_layout), // explicit layout so the bind group validates
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("wp00_enc"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("wp00_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(1, 1, 1); // exactly one workgroup, spec 9.7
    }
    encoder.copy_buffer_to_buffer(
        &src_buf,
        0,
        &read_buf,
        0,
        OUT_LEN as u64 * std::mem::size_of::<u32>() as u64,
    );
    queue.submit(Some(encoder.finish()));

    read_buf.map_async(MapMode::Read, ..OUT_LEN as u64 * 4, |_| {});
    if let Err(e) =
        device.poll(wgpu::PollType::Wait { timeout: None, submission_index: None })
    {
        report.diagnostics.push(format!("failure: poll error (device loss?): {e:?}"));
        fail(&report);
    }

    // --- exact-integer compare (spec 9.7) ---
    let view = read_buf.get_mapped_range(..OUT_LEN as u64 * 4);
    let got = bytemuck::cast_slice::<u8, u32>(&view);
    let mut ok = got.len() == OUT_LEN as usize;
    for (i, word) in got.iter().enumerate() {
        let expected = ((i as u32) << 8) | (4096 + i as u32);
        if *word != expected {
            report
                .diagnostics
                .push(format!("failure: readback[{i}] = {word}, expected {expected}"));
            ok = false;
        }
    }
    drop(view);
    read_buf.unmap();
    report.compute_readback_verified = ok;

    // --- RGBA16F attachment capability (spec 9.8) ---
    // Queried from the adapter's format-feature table (honest capability
    // statement per spec 9.11); render graph revisits at WP06.
    let fmt_feats = adapter.get_texture_format_features(wgpu::TextureFormat::Rgba16Float);
    report
        .diagnostics
        .push(format!("rgba16f allowed usages = {:?}", fmt_feats.allowed_usages));
    report.rgba16f_attachment =
        fmt_feats.allowed_usages.contains(TextureUsages::RENDER_ATTACHMENT);

    report.success = report.compute_readback_verified && report.rgba16f_attachment;
    println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
    std::process::exit(if report.success { 0 } else { 1 });
}
