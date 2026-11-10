//! Renderer device/session (§15.8, §15.9): target lifecycle, per-frame
//! orchestration of the pinned `MAIN_PASS_SEQUENCE`, and the
//! `create_renderer` / `resize` / `render_scene` surface consumed by
//! anim_app and anim_export.
//!
//! D-11: NO automatic device/adapter selection — `create_renderer`
//! receives caller-provided `Device` + `Queue` and only validates the
//! §15.6 REQUIRED formats (typed `MissingRequiredFormat`, never a
//! silent reformat).
//!
//! History: the renderer owns a `PreviousFrameState` that callers seed
//! (`history_mut`) and read (`previous_frame_state`) for pure
//! `extract_frame` calls; `render_scene` commits it after every
//! successful submission.

use crate::camera::HistoryInvalidation;
use crate::draw_list::{PreviousFrameState, RenderFrame};
use crate::error::{RenderError, RenderResult};
use crate::graph::PostSettings;
use crate::instance::{InstanceData, QUAD_VERTICES};
use crate::memory::{MemoryError, MemoryManager};
use crate::passes::opaque;
use crate::passes::ordered_2d;
use crate::passes::output;
use crate::passes::post::{self, PostBinds, PostTextures};
use crate::passes::transparent;
use crate::passes::{PassContext, Pipelines, TargetViews};
use crate::targets::{self, RenderTargets};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt as _;

/// Camera uniform twin of the WGSL `CameraBlock` (128 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
struct CameraBlock {
    current: [[f32; 4]; 4],
    previous: [[f32; 4]; 4],
}

unsafe impl Pod for CameraBlock {}
unsafe impl Zeroable for CameraBlock {}

/// WGSL `PostParams` twin (16 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
struct PostParamsBytes {
    p0: f32,
    p1: f32,
    p2: f32,
    p3: f32,
}

unsafe impl Pod for PostParamsBytes {}
unsafe impl Zeroable for PostParamsBytes {}

/// WGSL `OutputParams` twin (16 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
struct OutputParamsBytes {
    transfer_manual: f32,
    pad_y: f32,
    pad_z: f32,
    pad_w: f32,
}

unsafe impl Pod for OutputParamsBytes {}
unsafe impl Zeroable for OutputParamsBytes {}

/// One small constant uniform + its group-0 bind group.
struct ParamsUniform {
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl ParamsUniform {
    fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        label: &'static str,
    ) -> Self {
        let buffer = crate::passes::tiny_uniform(
            device,
            bytemuck::bytes_of(&PostParamsBytes { p0: 0.0, p1: 0.0, p2: 0.0, p3: 0.0 })
                .try_into()
                .expect("16-byte params"),
            label,
        );
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        Self { buffer, bind_group }
    }

    fn write<P: Pod>(&self, queue: &wgpu::Queue, params: P) {
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&params));
    }
}

/// Result of one successful `render_scene` (§15.9).
pub struct RenderedScene {
    /// Output target texture (caller decides surface presentation or
    /// further export handling — §15.10).
    pub texture: wgpu::Texture,
    pub format: wgpu::TextureFormat,
    pub extent: wgpu::Extent3d,
    /// Target generation of the targets rendered against.
    pub generation: u64,
    /// Executed main passes, in order (G5 command-log verification).
    pub main_pass_log: Vec<crate::graph::MainPass>,
    /// Executed post sub-pass names, in order.
    pub post_log: Vec<&'static str>,
}

impl RenderedScene {
    /// PADDED readback of the output texture (typically Rgba8*).
    pub fn readback_output(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> RenderResult<Vec<u8>> {
        crate::readback::download_texture(
            device,
            queue,
            &self.texture,
            "anim_render.readback.output",
        )
    }

    /// PADDED readback of a raw HDR target (used by the G5 goldens:
    /// linear decode and HDR-before-tone-map verification).
    pub fn readback_source(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> RenderResult<Vec<u8>> {
        crate::readback::download_texture(
            device,
            queue,
            &self.texture,
            "anim_render.readback.texture",
        )
    }

    /// Tight (unpadded) output bytes in `format`'s native byte order.
    pub fn readback_output_unpadded(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> RenderResult<Vec<u8>> {
        let padded = self.readback_output(device, queue)?;
        let bpp = crate::readback::block_bytes(self.format);
        crate::readback::unpad_rows(&padded, bpp, self.extent.width, self.extent.height)
    }
}

/// The WP06 renderer session. Targets and pipelines are owned here;
/// the `Device`/`Queue` are borrowed per call (D-11).
pub struct Renderer {
    /// GPU memory manager (§16 WP07): tracks residency, budgets, serials.
    memory: MemoryManager,
    targets: RenderTargets,
    output: wgpu::Texture,
    output_format: wgpu::TextureFormat,
    generation: u64,
    /// Bookkeeping the caller seeds/reads for pure extraction.
    pub(crate) frame_state: PreviousFrameState,

    pipelines: Pipelines,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    quad: wgpu::Buffer,
    // post chain params (constant-sized uniforms, rewritten per frame)
    params_motion: ParamsUniform,
    params_bloom_bright: ParamsUniform,
    params_bloom_v: ParamsUniform,
    params_bloom_combine: ParamsUniform,
    params_tone: ParamsUniform,
    params_grade: ParamsUniform,
    params_output: ParamsUniform,
}

/// Validate that a format is attachable (typed — §15.11). wgpu 27 has
/// no device-side feature query, so this performs a tiny probe texture
/// creation inside a validation error scope and fails TYPED on any
/// validation error — never silently reformats.
fn require_attachable(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> RenderResult<()> {
    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let probe = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("anim_render.format_probe"),
        size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    });
    drop(probe);
    let _ = device.poll(wgpu::PollType::Poll);
    let caught = pollster::block_on(device.pop_error_scope());
    match caught {
        Some(e) => Err(RenderError::MissingRequiredFormat(format!(
            "format {format:?} unsupported for usage {usage:?}: {e}"
        ))),
        None => Ok(()),
    }
}

/// Internal frame targets selectable for the G5 diagnostic readback.
/// Exposed deliberately (spec §15.11 LOUD diagnostics); NOT a rendering
/// API — the render path never depends on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InternalTarget {
    /// Linear scene-referred HDR after the (velocity-null = identity)
    /// motion-blur copy — the PRE-tone-map stack anchor for goldens.
    SceneHdr,
    /// View-space normal xyz + validity w.
    Normal,
    /// Screen-space velocity xy + validity z (reserved w = 0).
    Velocity,
    /// Post ping-pong slot 0 (post-grade state ends in slot 1 for the
    /// default chain).
    Post0,
    /// Post ping-pong slot 1 (graded result for the default chain).
    Post1,
}

impl Renderer {
    /// D-11 factory: validate REQUIRED formats against the CALLER'S
    /// device, build pipelines and allocate the pinned target set.
    pub fn create(
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        config: &crate::RenderConfig,
    ) -> RenderResult<Self> {
        // Pinned internal formats must all be attachable (+ sampleable
        // for the HDR ones the post chain reads back).
        for format in targets::format::REQUIRED {
            require_attachable(
                device,
                *format,
                wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
            )?;
        }
        // Output format explicitly configured by the caller.
        require_attachable(
            device,
            config.output_format,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
        )?;

        let pipelines = Pipelines::build(device, config.output_format);
        let targets = RenderTargets::create(device, config.extent)?;
        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("anim_render.output"),
            size: config.extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: config.output_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("anim_render.camera"),
            size: std::mem::size_of::<CameraBlock>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("anim_render.camera.bg"),
            layout: &pipelines.scene.camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        let quad = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("anim_render.quad"),
            contents: bytemuck::cast_slice(QUAD_VERTICES),
            usage: wgpu::BufferUsages::VERTEX,
        });

        // Placeholder instance buffer for registration (real one per-frame).
        let instance_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("anim_render.instances.placeholder"),
                contents: bytemuck::bytes_of(&<InstanceData as Zeroable>::zeroed()),
                usage: wgpu::BufferUsages::STORAGE,
            });

        // Memory manager: starts with defaults; caller may inject
        // policy/telemetry via memory mutator methods after creation.
        let mut memory = MemoryManager::new();
        register_renderer_resources(
            &mut memory,
            &targets,
            &output,
            &camera_buffer,
            &instance_buffer,
        )
        .map_err(|e| e.to_render_error())?;

        let params_motion = ParamsUniform::new(
            device,
            &pipelines.post.params_layout,
            "anim_render.params.motion",
        );
        let params_bloom_bright = ParamsUniform::new(
            device,
            &pipelines.post.params_layout,
            "anim_render.params.bloom_bright",
        );
        let params_bloom_v = ParamsUniform::new(
            device,
            &pipelines.post.params_layout,
            "anim_render.params.bloom_v",
        );
        let params_bloom_combine = ParamsUniform::new(
            device,
            &pipelines.post.params_layout,
            "anim_render.params.bloom_combine",
        );
        let params_tone = ParamsUniform::new(
            device,
            &pipelines.post.params_layout,
            "anim_render.params.tone",
        );
        let params_grade = ParamsUniform::new(
            device,
            &pipelines.post.params_layout,
            "anim_render.params.grade",
        );
        let params_output = ParamsUniform::new(
            device,
            &pipelines.output.params_layout,
            "anim_render.params.output",
        );

        Ok(Self {
            memory,
            targets,
            output,
            output_format: config.output_format,
            generation: 0,
            frame_state: PreviousFrameState::fresh(),
            pipelines,
            camera_buffer,
            camera_bind_group,
            quad,
            params_motion,
            params_bloom_bright,
            params_bloom_v,
            params_bloom_combine,
            params_tone,
            params_grade,
            params_output,
        })
    }

    pub fn output_format(&self) -> wgpu::TextureFormat {
        self.output_format
    }

    /// Immutable metrics copy for diagnostics / budget report.
    pub fn memory(&self) -> &MemoryManager {
        &self.memory
    }

    /// Mutable access for frame-time telemetry sampling (budget report
    /// consumes this between frames).
    pub fn memory_mut(&mut self) -> &mut MemoryManager {
        &mut self.memory
    }

    /// G5 diagnostic readback of one internal frame target (padded
    /// rows; decode with `readback::unpad_rows` + f16 conversion).
    pub fn readback_internal(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: InternalTarget,
    ) -> RenderResult<Vec<u8>> {
        let texture = match target {
            InternalTarget::SceneHdr => &self.targets.scene_hdr,
            InternalTarget::Normal => &self.targets.normal,
            InternalTarget::Velocity => &self.targets.velocity,
            InternalTarget::Post0 => &self.targets.post_hdr[0],
            InternalTarget::Post1 => &self.targets.post_hdr[1],
        };
        crate::readback::download_texture(
            device,
            queue,
            texture,
            "anim_render.readback.internal",
        )
    }

    pub fn targets_generation(&self) -> u64 {
        self.generation
    }

    pub fn extent(&self) -> wgpu::Extent3d {
        self.targets.extent
    }

    /// Read-only view of the caller-seeded extraction history.
    pub fn previous_frame_state(&self) -> &PreviousFrameState {
        &self.frame_state
    }

    /// Read-only view for readback goldens (§15.11).
    pub fn readback(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
    ) -> RenderResult<Vec<u8>> {
        crate::readback::download_texture(device, queue, texture, "anim_render.readback")
    }

    /// Caller invalidation hook (load, seek, teleport, …) BEFORE
    /// extraction. The renderer keeps its own serials consistent.
    pub fn invalidate_history(&mut self, reason: &HistoryInvalidation) {
        self.frame_state.history.invalidate(reason);
    }

    /// Atomic resize (§15.8/§15.11): zero extents are REJECTED keeping
    /// the last valid set, otherwise ALL targets (incl. output) are
    /// recreated and the generation bumps. An aspect change
    /// additionally invalidates history as `ProjectionChange`.
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        extent: wgpu::Extent3d,
    ) -> RenderResult<()> {
        targets::validate_extent(extent)?;
        let aspect = f64::from(extent.width) / f64::from(extent.height);
        let aspect_changed =
            (aspect - self.frame_state.viewport_aspect).abs() > f64::EPSILON * 1_000.0;
        self.targets = RenderTargets::create(device, extent)?;
        self.output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("anim_render.output"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.output_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        self.generation = self.generation.wrapping_add(1);
        if aspect_changed {
            self.frame_state.viewport_aspect = aspect;
            self.frame_state.history.invalidate(&HistoryInvalidation::ProjectionChange);
        }
        self.frame_state.history.invalidate(&HistoryInvalidation::TargetResize);

        // Re-register all GPU resources at the new extent (new textures
        // = new residency; the old records are orphaned and will be
        // evicted naturally by the memory manager's serial gating).
        let instance_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("anim_render.instances.placeholder"),
                contents: bytemuck::bytes_of(&<InstanceData as Zeroable>::zeroed()),
                usage: wgpu::BufferUsages::STORAGE,
            });
        register_renderer_resources(
            &mut self.memory,
            &self.targets,
            &self.output,
            &self.camera_buffer,
            &instance_buffer,
        )
        .map_err(|e| e.to_render_error())?;

        Ok(())
    }

    /// Render one fully-prepared frame: draw list already extracted and
    /// sorted (`RenderFrame.items` from `extract_frame`), camera pair
    /// resolved, `cut` decided. Returns the output target + logs.
    pub fn render_scene(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &RenderFrame,
    ) -> RenderResult<RenderedScene> {
        // ---- 1. instance upload (sorted order = slot order) ----
        let instances: Vec<InstanceData> =
            frame.items.iter().map(InstanceData::from_item).collect();

        // ---- 2. camera uniform ----
        let camera = CameraBlock {
            current: crate::instance::to_cols(frame.camera.current_view_proj),
            previous: crate::instance::to_cols(frame.camera.previous_view_proj),
        };
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&camera));

        // ---- 3. post params (pinned §15.7 profile, ADR-0010) ----
        let settings = PostSettings::default();
        self.params_motion.write(
            queue,
            PostParamsBytes {
                p0: 4.0, // 4 sample steps along the velocity
                p1: 0.0,
                p2: 0.0,
                p3: 0.0,
            },
        );
        self.params_bloom_bright.write(
            queue,
            PostParamsBytes {
                p0: 1.0, // HDR threshold
                p1: 1.0, // gain
                p2: 1.0, // tap radius
                p3: 0.0,
            },
        );
        self.params_bloom_v
            .write(queue, PostParamsBytes { p0: 0.0, p1: 0.0, p2: 1.0, p3: 0.0 });
        self.params_bloom_combine.write(
            queue,
            PostParamsBytes {
                p0: 0.0,
                p1: 0.0,
                p2: 0.0,
                p3: 1.0, // bloom strength
            },
        );
        self.params_tone
            .write(queue, PostParamsBytes { p0: 0.0, p1: 0.0, p2: 0.0, p3: 0.0 });
        self.params_grade
            .write(queue, PostParamsBytes { p0: 0.0, p1: 0.0, p2: 0.0, p3: 0.0 });
        self.params_output.write(
            queue,
            OutputParamsBytes {
                // sRGB-aware target → hardware encodes exactly once.
                transfer_manual: if is_srgb_aware(self.output_format) { 0.0 } else { 1.0 },
                pad_y: 0.0,
                pad_z: 0.0,
                pad_w: 0.0,
            },
        );

        // ---- 4. per-frame bind groups ----
        let instance_buffer = if instances.is_empty() {
            // An EMPTY storage binding still satisfies min_binding_size.
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("anim_render.instances.empty"),
                contents: bytemuck::bytes_of(&<InstanceData as Zeroable>::zeroed()),
                usage: wgpu::BufferUsages::STORAGE,
            })
        } else {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("anim_render.instances"),
                contents: bytemuck::cast_slice(&instances),
                usage: wgpu::BufferUsages::STORAGE,
            })
        };
        let _ = &self.quad; // vertex data is WGSL-const; buffer kept for future paths
        let instance_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("anim_render.instances.bg"),
            layout: &self.pipelines.scene.instances_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: instance_buffer.as_entire_binding(),
            }],
        });

        let views = TargetViews::from_textures(&self.targets, &self.output);

        macro_rules! tex_bg {
            ($label:literal, $layout:expr, $view:expr) => {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some($label),
                    layout: $layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView($view),
                    }],
                })
            };
        }

        let post_binds = PostBinds {
            params_motion: &self.params_motion.bind_group,
            params_bloom_bright: &self.params_bloom_bright.bind_group,
            params_bloom_v: &self.params_bloom_v.bind_group,
            params_bloom_combine: &self.params_bloom_combine.bind_group,
            params_tone: &self.params_tone.bind_group,
            params_grade: &self.params_grade.bind_group,
            motion_g1_scene_g2_velocity: (
                &tex_bg!(
                    "anim_render.bg.motion.a",
                    &self.pipelines.post.tex_layout,
                    &views.scene_hdr
                ),
                &tex_bg!(
                    "anim_render.bg.motion.b",
                    &self.pipelines.post.tex_layout,
                    &views.velocity
                ),
            ),
            bright_g1_scene: &tex_bg!(
                "anim_render.bg.bright.a",
                &self.pipelines.post.tex_layout,
                &views.scene_hdr
            ),
            bloom_v_g1_post1: &tex_bg!(
                "anim_render.bg.bloom_v.a",
                &self.pipelines.post.tex_layout,
                &views.post[1]
            ),
            combine_g1_scene_g2_post0: (
                &tex_bg!(
                    "anim_render.bg.combine.a",
                    &self.pipelines.post.tex_layout,
                    &views.scene_hdr
                ),
                &tex_bg!(
                    "anim_render.bg.combine.b",
                    &self.pipelines.post.tex_layout,
                    &views.post[0]
                ),
            ),
            tone_g1_scene: &tex_bg!(
                "anim_render.bg.tone.scene",
                &self.pipelines.post.tex_layout,
                &views.scene_hdr
            ),
            tone_g1_post1: &tex_bg!(
                "anim_render.bg.tone.post1",
                &self.pipelines.post.tex_layout,
                &views.post[1]
            ),
            grade_g1_post0: &tex_bg!(
                "anim_render.bg.grade.post0",
                &self.pipelines.post.tex_layout,
                &views.post[0]
            ),
        };

        // Output reads post[1] after grade, post[0] when grade is off.
        let post_final_view =
            if settings.grade_enabled { &views.post[1] } else { &views.post[0] };
        let output_binds = output::OutputBinds {
            params: &self.params_output.bind_group,
            g1_read: &tex_bg!(
                "anim_render.bg.output.a",
                &self.pipelines.output.tex_layout,
                post_final_view
            ),
        };

        // ---- 5. encode the pinned MAIN_PASS_SEQUENCE ----
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("anim_render.frame"),
        });

        // §16.7: assign submission serial, mark all frame-used resources.
        let submission_serial = self.memory.begin_submission();
        // All registered renderer resources are used every frame.
        for (id, _) in self.memory.records() {
            self.memory.mark_used(id, submission_serial, frame.tick.0 as u64);
        }

        let mut main_log: Vec<crate::graph::MainPass> = Vec::with_capacity(5);
        let mut post_log: Vec<&'static str> = Vec::with_capacity(5);

        {
            let mut ctx = PassContext {
                device,
                pipeline_info: &self.pipelines,
                camera_bg: &self.camera_bind_group,
                instance_bg: &instance_bg,
                views: &views,
                items: &frame.items,
                instances: &instances,
                pass_log: &mut main_log,
            };
            opaque::run(&mut ctx, &mut encoder)?;
            ordered_2d::run(&mut ctx, &mut encoder)?;
            transparent::run(&mut ctx, &mut encoder)?;
        }

        let textures = PostTextures {
            scene_hdr: &self.targets.scene_hdr,
            post: [&self.targets.post_hdr[0], &self.targets.post_hdr[1]],
        };
        let mut post_run = post::PostRun {
            pipelines: &self.pipelines,
            settings: &settings,
            extent: self.targets.extent,
            textures: &textures,
            binds: post_binds,
            views: &views,
            main_log: &mut main_log,
            post_log: &mut post_log,
        };
        post::run(&mut post_run, &mut encoder)?;
        // post_run's log borrows end when this scope closes; main_log /
        // post_log are consumed by RenderedScene below.

        output::run(
            &self.pipelines.output,
            !is_srgb_aware(self.output_format),
            &output_binds,
            &views,
            &mut encoder,
            &mut main_log,
        );

        // ---- 6. submit + commit history ----
        queue.submit(Some(encoder.finish()));
        // §16.7 fence lifecycle: submission recorded.
        self.memory.on_submitted(submission_serial);

        // Poll for completion to record the fence (wgpu has no native
        // fence callback; block until the last submitted work completes).
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| RenderError::InvalidSnapshot(format!("completion poll: {e}")))?;
        self.memory.on_completed(submission_serial);

        self.frame_state.record_successful_submission(frame, self.generation);

        Ok(RenderedScene {
            texture: self.output.clone(),
            format: self.output_format,
            extent: self.targets.extent,
            generation: self.generation,
            main_pass_log: main_log,
            post_log,
        })
    }

    /// CPU-mirror helper for tests: items → instance bytes.
    #[cfg(test)]
    #[allow(dead_code)] // used by the G5 golden tests when they land (chunk 3)
    pub(crate) fn instances_of(items: &[crate::draw_list::DrawItem]) -> Vec<InstanceData> {
        items.iter().map(InstanceData::from_item).collect()
    }
}

/// sRGB-aware target check (§15.7): the transfer is performed by the
/// target's own format exactly once when it is sRGB.
fn is_srgb_aware(format: wgpu::TextureFormat) -> bool {
    format.is_srgb()
}

/// Compute GPU byte size of a texture (extent × format block size).
fn texture_bytes(extent: wgpu::Extent3d, format: wgpu::TextureFormat) -> u64 {
    let block =
        wgpu::TextureFormat::block_copy_size(&format, Some(wgpu::TextureAspect::All))
            .unwrap_or(4) as u64;
    u64::from(extent.width)
        * u64::from(extent.height)
        * u64::from(extent.depth_or_array_layers)
        * block
}

/// Register all renderer-owned GPU resources with the memory manager.
fn register_renderer_resources(
    memory: &mut MemoryManager,
    targets: &RenderTargets,
    output: &wgpu::Texture,
    camera_buffer: &wgpu::Buffer,
    instance_buffer: &wgpu::Buffer,
) -> Result<(), MemoryError> {
    let extent = targets.extent;
    // scene_hdr (Rgba16Float = 8 bytes/pixel)
    memory.register_resource(crate::memory::resource::ResourceRecord {
        id: crate::memory::resource::ResourceId::PLACEHOLDER,
        label: "scene_hdr".into(),
        bytes: texture_bytes(extent, targets::format::SCENE_HDR),
        source: crate::memory::resource::ContentSource::Regenerable,
        priority: crate::memory::resource::ResourcePriority::CurrentFrame,
        last_used_serial: 0,
        last_used_frame: 0,
        resident: true,
        locked: false,
        reload_state: crate::memory::resource::ReloadState::Resident,
    })?;
    // normal (Rgba16Float)
    memory.register_resource(crate::memory::resource::ResourceRecord {
        id: crate::memory::resource::ResourceId::PLACEHOLDER,
        label: "normal".into(),
        bytes: texture_bytes(extent, targets::format::NORMAL),
        source: crate::memory::resource::ContentSource::Regenerable,
        priority: crate::memory::resource::ResourcePriority::CurrentFrame,
        last_used_serial: 0,
        last_used_frame: 0,
        resident: true,
        locked: false,
        reload_state: crate::memory::resource::ReloadState::Resident,
    })?;
    // velocity (Rgba16Float)
    memory.register_resource(crate::memory::resource::ResourceRecord {
        id: crate::memory::resource::ResourceId::PLACEHOLDER,
        label: "velocity".into(),
        bytes: texture_bytes(extent, targets::format::VELOCITY),
        source: crate::memory::resource::ContentSource::Regenerable,
        priority: crate::memory::resource::ResourcePriority::CurrentFrame,
        last_used_serial: 0,
        last_used_frame: 0,
        resident: true,
        locked: false,
        reload_state: crate::memory::resource::ReloadState::Resident,
    })?;
    // post_hdr[0] (Rgba16Float)
    memory.register_resource(crate::memory::resource::ResourceRecord {
        id: crate::memory::resource::ResourceId::PLACEHOLDER,
        label: "post_hdr[0]".into(),
        bytes: texture_bytes(extent, targets::format::POST_HDR),
        source: crate::memory::resource::ContentSource::Regenerable,
        priority: crate::memory::resource::ResourcePriority::CurrentFrame,
        last_used_serial: 0,
        last_used_frame: 0,
        resident: true,
        locked: false,
        reload_state: crate::memory::resource::ReloadState::Resident,
    })?;
    // post_hdr[1] (Rgba16Float)
    memory.register_resource(crate::memory::resource::ResourceRecord {
        id: crate::memory::resource::ResourceId::PLACEHOLDER,
        label: "post_hdr[1]".into(),
        bytes: texture_bytes(extent, targets::format::POST_HDR),
        source: crate::memory::resource::ContentSource::Regenerable,
        priority: crate::memory::resource::ResourcePriority::CurrentFrame,
        last_used_serial: 0,
        last_used_frame: 0,
        resident: true,
        locked: false,
        reload_state: crate::memory::resource::ReloadState::Resident,
    })?;
    // output (caller's format)
    memory.register_resource(crate::memory::resource::ResourceRecord {
        id: crate::memory::resource::ResourceId::PLACEHOLDER,
        label: "output".into(),
        bytes: texture_bytes(output.size(), output.format()),
        source: crate::memory::resource::ContentSource::Regenerable,
        priority: crate::memory::resource::ResourcePriority::CurrentFrame,
        last_used_serial: 0,
        last_used_frame: 0,
        resident: true,
        locked: false,
        reload_state: crate::memory::resource::ReloadState::Resident,
    })?;
    // depth (Depth32Float = 4 bytes/pixel, no texture binding)
    memory.register_resource(crate::memory::resource::ResourceRecord {
        id: crate::memory::resource::ResourceId::PLACEHOLDER,
        label: "depth".into(),
        bytes: texture_bytes(extent, targets::format::DEPTH),
        source: crate::memory::resource::ContentSource::Regenerable,
        priority: crate::memory::resource::ResourcePriority::CurrentFrame,
        last_used_serial: 0,
        last_used_frame: 0,
        resident: true,
        locked: false,
        reload_state: crate::memory::resource::ReloadState::Resident,
    })?;
    // camera uniform buffer (128 bytes)
    memory.register_resource(crate::memory::resource::ResourceRecord {
        id: crate::memory::resource::ResourceId::PLACEHOLDER,
        label: "camera_uniform".into(),
        bytes: camera_buffer.size(),
        source: crate::memory::resource::ContentSource::Regenerable,
        priority: crate::memory::resource::ResourcePriority::CurrentFrame,
        last_used_serial: 0,
        last_used_frame: 0,
        resident: true,
        locked: false,
        reload_state: crate::memory::resource::ReloadState::Resident,
    })?;
    // instance storage buffer (per-frame allocation; register with
    // placeholder bytes, updated at first render)
    memory.register_resource(crate::memory::resource::ResourceRecord {
        id: crate::memory::resource::ResourceId::PLACEHOLDER,
        label: "instance_storage".into(),
        bytes: instance_buffer.size(),
        source: crate::memory::resource::ContentSource::Regenerable,
        priority: crate::memory::resource::ResourcePriority::CurrentFrame,
        last_used_serial: 0,
        last_used_frame: 0,
        resident: true,
        locked: false,
        reload_state: crate::memory::resource::ReloadState::Resident,
    })?;
    Ok(())
}
