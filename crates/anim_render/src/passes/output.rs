//! Output transfer pass (§15.7): final display-encoded render.
//!
//! "Tone mapping runs once. Display-referred grading and output
//! transfer follow their declared profile." This is the LAST pass of
//! the frame: input = graded post_hdr[1] (post tone map + grade);
//! output = the configured output texture (surface or export target).
//!
//! Transfer contract (G5 "exactly one output transfer"):
//!   target format sRGB-aware (describe().srgb == true)  → shader emits
//!     LINEAR (transfer_manual = 0.0); the hardware performs the encode
//!     exactly once in the color attachment write.
//!   target format linearly encoded (Rgba8Unorm, …)      → shader
//!     performs the manual linear→sRGB encode exactly once (p0 = 1.0).
//!
//! NO premultiplied→straight conversion here (never silently applied —
//! anim_export/WP09 owns it when a codec requires it).

use crate::graph::MainPass;
use crate::passes::post::fs_pass;
use crate::passes::{OutputPipeline, TargetViews};
use wgpu::CommandEncoder;

/// Per-frame output bind groups.
pub(crate) struct OutputBinds<'a> {
    pub params: &'a wgpu::BindGroup,
    pub g1_read: &'a wgpu::BindGroup,
}

/// Draw the transfer from post_hdr[1] into the output target.
pub(crate) fn run(
    output: &OutputPipeline,
    transfer_manual: bool,
    binds: &OutputBinds,
    views: &TargetViews,
    encoder: &mut CommandEncoder,
    main_log: &mut Vec<MainPass>,
) {
    fs_pass(
        encoder,
        "anim_render.pass.output_transfer",
        &output.transfer,
        binds.params,
        binds.g1_read,
        None,
        &views.output,
    );
    let _ = transfer_manual; // contract lives in the params buffer content (device.rs)
    main_log.push(MainPass::OutputTransfer);
}
