//! Explicit 2D layer-order pass (§15.6).
//!
//! Painter's algorithm over the CPU's stable total sort. Single HDR
//! target with Load (the opaque pass cleared and possibly drew it),
//! depth TEST ONLY (LessEqual, no write — coplanar 2D ties both draw).

use super::{draw_item_slot, pipeline_for, PassContext};
use crate::error::RenderResult;
use crate::graph::{MainPass, MATERIAL_ORDERED_2D};

pub(crate) fn run(
    ctx: &mut PassContext,
    encoder: &mut wgpu::CommandEncoder,
) -> RenderResult<()> {
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("anim_render.pass.ordered_2d"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &ctx.views.scene_hdr,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &ctx.views.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    // STORE, not Discard: the transparent pass LOADs this
                    // depth afterwards — Load after Discard is undefined
                    // content (observed: fragments z-fail on the RTX).
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        let mut last_pipeline: Option<&wgpu::RenderPipeline> = None;
        for (slot, item) in ctx.items.iter().enumerate() {
            if item.material_pass != MATERIAL_ORDERED_2D {
                continue;
            }
            let pipeline = pipeline_for(ctx.pipeline_info, item)?;
            if last_pipeline != Some(pipeline) {
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, ctx.camera_bg, &[]);
                pass.set_bind_group(1, ctx.instance_bg, &[]);
                last_pipeline = Some(pipeline);
            }
            draw_item_slot(&mut pass, slot);
        }
    }
    ctx.pass_log.push(MainPass::Ordered2D);
    Ok(())
}
