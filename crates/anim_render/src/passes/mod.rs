//! Concrete pass implementations and shared pipeline plumbing (§15.3).
//!
//! Bind model:
//!   group 0 (scene passes): CameraBlock uniform
//!   group 1 (scene passes): instance array (storage read)
//!   group 0 (post/output): PostParams / OutputParams uniform
//!   group 1 (post/output): input texture A
//!   group 2 (post/output): input texture B (only blur/combine passes)
//!
//! All pass modules share the CORS quad (const in WGSL) — NO vertex
//! buffers are used; instance slots select draws via `instance_index`.

pub(crate) mod opaque;
pub(crate) mod ordered_2d;
pub(crate) mod output;
pub(crate) mod post;
pub(crate) mod transparent;

use crate::draw_list::DrawItem;
use crate::error::{RenderError, RenderResult};
use crate::graph::MainPass;
use crate::instance::InstanceData;
use wgpu::util::DeviceExt as _;

/// Shared transparent clear color for every HDR attachment.
pub(crate) const CLEAR_TRANSPARENT: wgpu::Color =
    wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };

/// Depth state used by the blended 2D passes (test-only, LessEqual).
fn depth_test_state() -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: crate::targets::format::DEPTH,
        depth_write_enabled: false,
        // Coplanar 2D content must BOTH draw (ties resolved by the
        // stable sort + draw order) — LessEqual is the painter rule.
        depth_compare: wgpu::CompareFunction::LessEqual,
        stencil: Default::default(),
        bias: Default::default(),
    }
}

pub(crate) const SHADER_SCENE: &str = include_str!("../shaders/scene.wgsl");
pub(crate) const SHADER_ORDERED_2D: &str = include_str!("../shaders/ordered_2d.wgsl");
pub(crate) const SHADER_TRANSPARENT: &str = include_str!("../shaders/transparent.wgsl");
pub(crate) const SHADER_POST: &str = include_str!("../shaders/post.wgsl");
pub(crate) const SHADER_OUTPUT: &str = include_str!("../shaders/output.wgsl");

// ---- bind group layouts ----

fn bgl_camera(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("anim_render.bgl.camera"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(128),
            },
            count: None,
        }],
    })
}

fn bgl_instances(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("anim_render.bgl.instances"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(160),
            },
            count: None,
        }],
    })
}

// Post/output uniform (fragment stage only).
fn bgl_params(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("anim_render.bgl.params"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(16),
            },
            count: None,
        }],
    })
}

fn bgl_texture(device: &wgpu::Device, label: &'static str) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    })
}

/// All pipelines pinned to the §15.6 target formats.
pub(crate) struct Pipelines {
    pub scene: ScenePipelines,
    pub post: PostPipelines,
    pub output: OutputPipeline,
}

pub(crate) struct ScenePipelines {
    /// Opaque (+ alpha-test) forward MRT: blend REPLACE, depth WRITE,
    /// compare Less.
    pub(crate) opaque: wgpu::RenderPipeline,
    /// Ordered 2D: blend over, depth test (LessEqual) NO write.
    pub ordered_2d: wgpu::RenderPipeline,
    /// Transparent over: One / OneMinusSrcAlpha, depth test no write.
    pub transparent_over: wgpu::RenderPipeline,
    /// Transparent additive: One / One, depth test no write.
    pub transparent_add: wgpu::RenderPipeline,

    pub camera_layout: wgpu::BindGroupLayout,
    pub instances_layout: wgpu::BindGroupLayout,
    #[allow(dead_code)] // kept for WP07 instrumentation (derived bind groups)
    scene_layout: wgpu::PipelineLayout,
}

impl ScenePipelines {
    /// Layout accessor (kept for WP07 instrumentation that needs to
    /// re-create derived bind group layouts).
    #[allow(dead_code)]
    pub(crate) fn layout(&self) -> &wgpu::PipelineLayout {
        &self.scene_layout
    }
}

pub(crate) struct PostPipelines {
    /// Scene HDR + velocity → post buffer.
    pub motion_blur: wgpu::RenderPipeline,
    /// Bloom bright+H layer → post buffer.
    pub bloom_bright: wgpu::RenderPipeline,
    /// Bloom vertical completion → post buffer.
    pub bloom_v: wgpu::RenderPipeline,
    /// base + bloom → post buffer (BOTH textures).
    pub bloom_combine: wgpu::RenderPipeline,
    /// Reinhard tone map (exactly once).
    pub tone_map: wgpu::RenderPipeline,
    /// Display-referred grade.
    pub grade: wgpu::RenderPipeline,

    pub params_layout: wgpu::BindGroupLayout,
    pub tex_layout: wgpu::BindGroupLayout,
    #[allow(dead_code)] // reserved for WP07+ instrumentation / derived layouts
    pub single_layout: wgpu::PipelineLayout,
    #[allow(dead_code)] // reserved for WP07+ instrumentation / derived layouts
    pub pair_layout: wgpu::PipelineLayout,
}

pub(crate) struct OutputPipeline {
    /// Final transfer into the configured output target.
    pub transfer: wgpu::RenderPipeline,
    pub params_layout: wgpu::BindGroupLayout,
    pub tex_layout: wgpu::BindGroupLayout,
    #[allow(dead_code)] // reserved for WP07+ instrumentation / derived layouts
    pub layout: wgpu::PipelineLayout,
}

fn module(
    device: &wgpu::Device,
    label: &'static str,
    src: &'static str,
) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(src)),
    })
}

fn pipeline(
    device: &wgpu::Device,
    label: &'static str,
    vs_entry: (&wgpu::ShaderModule, &'static str),
    fs_entry: (&wgpu::ShaderModule, &'static str),
    layout: &wgpu::PipelineLayout,
    targets: &[wgpu::ColorTargetState],
    depth: Option<wgpu::DepthStencilState>,
) -> wgpu::RenderPipeline {
    let owned_targets: Vec<Option<wgpu::ColorTargetState>> =
        targets.iter().map(|t| Some(t.clone())).collect();
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: vs_entry.0,
            entry_point: Some(vs_entry.1),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        },
        depth_stencil: depth,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: fs_entry.0,
            entry_point: Some(fs_entry.1),
            compilation_options: Default::default(),
            targets: &owned_targets,
        }),
        multiview: None,
        cache: None,
    })
}

impl Pipelines {
    /// Build every pipeline for the pinned §15.6 formats.
    pub fn build(device: &wgpu::Device, output_format: wgpu::TextureFormat) -> Self {
        let camera_layout = bgl_camera(device);
        let instances_layout = bgl_instances(device);
        let params_layout = bgl_params(device);
        let tex_layout = bgl_texture(device, "anim_render.bgl.tex");
        let scene_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("anim_render.layout.scene"),
            bind_group_layouts: &[&camera_layout, &instances_layout],
            push_constant_ranges: &[],
        });
        // Single-texture post passes: [params, input].
        let single_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("anim_render.layout.post_single"),
                bind_group_layouts: &[&params_layout, &tex_layout],
                push_constant_ranges: &[],
            });
        // Two-texture post passes (WGSL groups 1 and 2): [params, a, b].
        let pair_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("anim_render.layout.post_pair"),
            bind_group_layouts: &[&params_layout, &tex_layout, &tex_layout],
            push_constant_ranges: &[],
        });

        // scene shaders
        let scene_mod = module(device, "anim_render.shader.scene", SHADER_SCENE);
        let ordered_mod =
            module(device, "anim_render.shader.ordered_2d", SHADER_ORDERED_2D);
        let transparent_mod =
            module(device, "anim_render.shader.transparent", SHADER_TRANSPARENT);
        let post_mod = module(device, "anim_render.shader.post", SHADER_POST);
        let output_mod = module(device, "anim_render.shader.output", SHADER_OUTPUT);

        let hdr = crate::targets::format::SCENE_HDR;
        let no_blend: Option<wgpu::BlendState> = None;
        let blend_over =
            Some(crate::alpha::blend_state(crate::alpha::AlphaMode::Premultiplied));
        let blend_add =
            Some(crate::alpha::blend_state(crate::alpha::AlphaMode::AdditivePremultiplied));

        let depth_write = wgpu::DepthStencilState {
            format: crate::targets::format::DEPTH,
            depth_write_enabled: true,
            depth_compare: crate::targets::format::DEPTH_COMPARE,
            stencil: Default::default(),
            bias: Default::default(),
        };

        let opaque = pipeline(
            device,
            "anim_render.pipeline.opaque_mrt",
            (&scene_mod, "vs_main"),
            (&scene_mod, "fs_main"),
            &scene_layout,
            &[
                wgpu::ColorTargetState {
                    format: hdr,
                    blend: no_blend,
                    write_mask: wgpu::ColorWrites::ALL,
                },
                wgpu::ColorTargetState {
                    format: crate::targets::format::NORMAL,
                    blend: no_blend,
                    write_mask: wgpu::ColorWrites::ALL,
                },
                wgpu::ColorTargetState {
                    format: crate::targets::format::VELOCITY,
                    blend: no_blend,
                    write_mask: wgpu::ColorWrites::ALL,
                },
            ],
            Some(depth_write),
        );

        let target_hdr = wgpu::ColorTargetState {
            format: hdr,
            blend: blend_over,
            write_mask: wgpu::ColorWrites::ALL,
        };

        let ordered_2d = pipeline(
            device,
            "anim_render.pipeline.ordered_2d",
            (&ordered_mod, "vs_main"),
            (&ordered_mod, "fs_main"),
            &scene_layout,
            std::slice::from_ref(&target_hdr),
            Some(depth_test_state()),
        );

        let transparent_over = pipeline(
            device,
            "anim_render.pipeline.transparent_over",
            (&transparent_mod, "vs_main"),
            (&transparent_mod, "fs_main"),
            &scene_layout,
            std::slice::from_ref(&target_hdr),
            Some(depth_test_state()),
        );

        let transparent_add = pipeline(
            device,
            "anim_render.pipeline.transparent_add",
            (&transparent_mod, "vs_main"),
            (&transparent_mod, "fs_main"),
            &scene_layout,
            // NOTE: additive blend on the alpha channel differs from
            // `blend_add.alpha`? NO — blend_state maps the SAME factors
            // for color AND alpha (documented alpha-exactness rule).
            &[wgpu::ColorTargetState {
                format: hdr,
                blend: blend_add,
                write_mask: wgpu::ColorWrites::ALL,
            }],
            Some(depth_test_state()),
        );

        let mk_post = |label: &'static str, fs: &'static str, uses_pair: bool| {
            pipeline(
                device,
                label,
                (&post_mod, "vs_fullscreen"),
                (&post_mod, fs),
                if uses_pair { &pair_layout } else { &single_layout },
                &[wgpu::ColorTargetState {
                    format: crate::targets::format::POST_HDR,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                }],
                None,
            )
        };

        let post = PostPipelines {
            motion_blur: mk_post(
                "anim_render.pipeline.post.motion_blur",
                "fs_motion_blur",
                true,
            ),
            bloom_bright: mk_post(
                "anim_render.pipeline.post.bloom_bright",
                "fs_bloom_bright",
                false,
            ),
            bloom_v: mk_post("anim_render.pipeline.post.bloom_v", "fs_bloom_v", false),
            bloom_combine: mk_post(
                "anim_render.pipeline.post.bloom_combine",
                "fs_bloom_combine",
                true,
            ),
            tone_map: mk_post("anim_render.pipeline.post.tone_map", "fs_tone_map", false),
            grade: mk_post("anim_render.pipeline.post.grade", "fs_grade", false),
            params_layout: params_layout.clone(),
            tex_layout: tex_layout.clone(),
            single_layout,
            pair_layout,
        };

        // Output layout built while the layouts are still alive (the
        // post struct consumed clones above).
        let output_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("anim_render.layout.output"),
                bind_group_layouts: &[&params_layout, &tex_layout],
                push_constant_ranges: &[],
            });
        let transfer = pipeline(
            device,
            "anim_render.pipeline.output.transfer",
            (&output_mod, "vs_main"),
            (&output_mod, "fs_main"),
            &output_layout,
            &[wgpu::ColorTargetState {
                format: output_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            }],
            None,
        );
        let output =
            OutputPipeline { transfer, params_layout, tex_layout, layout: output_layout };

        Self {
            scene: ScenePipelines {
                opaque,
                ordered_2d,
                transparent_over,
                transparent_add,
                camera_layout,
                instances_layout,
                scene_layout,
            },
            post,
            output,
        }
    }
}

// ---- pass run context ----

/// Stable per-target views (recreated at resize; cached thereafter).
pub(crate) struct TargetViews {
    pub scene_hdr: wgpu::TextureView,
    pub depth: wgpu::TextureView,
    pub normal: wgpu::TextureView,
    pub velocity: wgpu::TextureView,
    pub post: [wgpu::TextureView; 2],
    pub output: wgpu::TextureView,
}

impl TargetViews {
    pub fn from_textures(
        targets: &crate::targets::RenderTargets,
        output: &wgpu::Texture,
    ) -> Self {
        Self {
            scene_hdr: targets.scene_hdr.create_view(&Default::default()),
            depth: targets.depth.create_view(&Default::default()),
            normal: targets.normal.create_view(&Default::default()),
            velocity: targets.velocity.create_view(&Default::default()),
            post: [
                targets.post_hdr[0].create_view(&Default::default()),
                targets.post_hdr[1].create_view(&Default::default()),
            ],
            output: output.create_view(&Default::default()),
        }
    }
}

/// Everything one pass needs; per-frame values are stale-free because
/// render_scene rebuilds per-frame bind groups before calling.
pub(crate) struct PassContext<'a> {
    #[allow(dead_code)] // passes take the device for future inline allocations
    pub device: &'a wgpu::Device,
    pub pipeline_info: &'a Pipelines,
    pub camera_bg: &'a wgpu::BindGroup,
    pub instance_bg: &'a wgpu::BindGroup,
    pub views: &'a TargetViews,
    /// ALL frame items (sorted); passes filter by material_pass.
    pub items: &'a [DrawItem],
    /// Parallel instances (same order as items; slot == index).
    #[allow(dead_code)] // kept in lock-step with items for WP07 diagnostics
    pub instances: &'a [InstanceData],
    /// Executed MAIN pass log (G5 command-log verification).
    pub pass_log: &'a mut Vec<MainPass>,
}

/// Which pipeline handles an item in a given pass.
pub(crate) fn pipeline_for<'a>(
    ctx_info: &'a Pipelines,
    item: &DrawItem,
) -> RenderResult<&'a wgpu::RenderPipeline> {
    use crate::alpha::AlphaMode;
    use crate::graph::{MATERIAL_OPAQUE_MRT, MATERIAL_ORDERED_2D, MATERIAL_TRANSPARENT};
    match item.material_pass {
        MATERIAL_OPAQUE_MRT => match item.alpha_mode {
            AlphaMode::Opaque | AlphaMode::AlphaTest { .. } => Ok(&ctx_info.scene.opaque),
            other => Err(RenderError::InvalidSnapshot(format!(
                "opaque MRT item with unsupported alpha mode {other:?}"
            ))),
        },
        MATERIAL_ORDERED_2D => match item.alpha_mode {
            AlphaMode::Straight | AlphaMode::Premultiplied => {
                Ok(&ctx_info.scene.ordered_2d)
            }
            other => Err(RenderError::InvalidSnapshot(format!(
                "ordered-2D item with unsupported alpha mode {other:?}"
            ))),
        },
        MATERIAL_TRANSPARENT => match item.alpha_mode {
            AlphaMode::AdditivePremultiplied => Ok(&ctx_info.scene.transparent_add),
            AlphaMode::Straight | AlphaMode::Premultiplied => {
                Ok(&ctx_info.scene.transparent_over)
            }
            other => Err(RenderError::InvalidSnapshot(format!(
                "transparent item with unsupported alpha mode {other:?}"
            ))),
        },
        other => Err(RenderError::InvalidGraph(format!(
            "unknown material pass selector {other}"
        ))),
    }
}

/// Indexed quad draw for item slot `slot` with the current pipeline.
pub(crate) fn draw_item_slot(pass: &mut wgpu::RenderPass<'_>, slot: usize) {
    use crate::instance::QUAD_VERTEX_COUNT;
    let s = slot as u32;
    pass.draw(0..QUAD_VERTEX_COUNT, s..s + 1);
}

// ---- small uniform helpers ----

/// Create a small uniform buffer initialized with `bytes` (16-byte).
pub(crate) fn tiny_uniform(
    device: &wgpu::Device,
    bytes: [u8; 16],
    label: &'static str,
) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: &bytes,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    })
}
