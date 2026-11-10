//! anim_render — device, graph, passes, GPU resources, memory policy and
//! readback (spec §15, WP06: Forward Render Graph, Color, Alpha, Velocity).
//!
//! WP06 progress (spec §15.5 sequence):
//! - DONE: typed errors, alpha modes/conversion/blend descriptors,
//!   camera matrices + history model, draw extraction + stable total
//!   ordering, target set declarations, pass graph declarations — pure
//!   and unit-tested.
//! - DONE (GPU half): WGSL shaders, pipelines, passes (opaque MRT /
//!   ordered 2D / transparent / post chain / output transfer),
//!   readback, device + render_scene orchestration.
//! - NEXT: tests/golden.rs (G5 gate), ADR-0010, STATUS update.
//!
//! Dependency direction (§15.4): anim_core, anim_assets, anim_scene,
//! anim_sim IN; anim_export/anim_editor consume this crate and are
//! NEVER dependencies of it.

#![deny(clippy::all)]
#![allow(clippy::multiple_crate_versions)] // wgpu/egui ecosystem transitive dup minors; revisit WP11

pub mod alpha;
pub mod camera;
pub mod device;
pub mod draw_list;
pub mod error;
pub mod graph;
pub mod instance;
pub mod memory;
pub(crate) mod passes;
pub mod readback;
pub mod targets;

pub use alpha::AlphaMode;
pub use camera::{CameraMatrices, HistoryInvalidation, HistoryState};
pub use device::{InternalTarget, RenderedScene, Renderer};
pub use draw_list::{
    extract_frame, sort_items_total, DrawItem, PreviousFrameState, RenderFrame,
};
pub use error::{RenderError, RenderResult};
pub use graph::{post_chain, MainPass, PostPass, PostSettings};
pub use instance::InstanceData;
pub use memory::{
    eviction::{compute_targets, BudgetSnapshot, EvictionPlan, Targets},
    fences::FenceTracker,
    identity::{AdapterIdentity, IdentityHints, IdentityState},
    metrics::{MetricCounters, MetricsSnapshot},
    resource::{ContentSource, MemoryPolicy, ResourceId, ResourcePriority, ResourceRecord},
    streaming::ReloadBatch,
    MemoryError, MemoryManager, SyntheticBudget,
};

/// Explicit render quality (§15.8). Export receives a FIXED resolved
/// profile — quality is never silently lowered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderQuality {
    Preview,
    Export,
}

/// Configuration handed to `create_renderer` (§15.9; consumed by the
/// GPU half). Output format is the CALLER'S decision (anim_app owns
/// surface config; anim_export owns export encoding — §15.10).
#[derive(Debug, Clone)]
pub struct RenderConfig {
    pub extent: wgpu::Extent3d,
    pub quality: RenderQuality,
    /// Explicit export or surface-compatible output format (§15.6).
    pub output_format: wgpu::TextureFormat,
}

/// Compiled frame plan the GPU half executes (declared in §15.5's
/// "concrete pass sequence"; construction is pure, validated here).
#[derive(Debug, Clone, PartialEq)]
pub struct FramePlan {
    pub mains: Vec<MainPass>,
    pub post: Vec<PostPass>,
}

/// Build and validate the pinned frame plan for a frame.
pub fn plan_frame(post_settings: &PostSettings) -> RenderResult<FramePlan> {
    Ok(FramePlan {
        mains: graph::MAIN_PASS_SEQUENCE.to_vec(),
        post: post_chain(post_settings)?,
    })
}

/// §15.9 interface — `create_renderer(device, queue, config)`.
/// D-11: the caller provides the device and queue; this crate never
/// selects adapters or falls back silently.
pub fn create_renderer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    config: &RenderConfig,
) -> RenderResult<Renderer> {
    device::Renderer::create(device, queue, config)
}
