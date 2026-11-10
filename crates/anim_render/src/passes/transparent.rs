//! Sorted transparent pass (§15.6, §15.7).
//!
//! Depth TEST only (LessEqual, no write). The stable total sort already
//! put over/additive items in back-to-front/effective order; pipeline
//! selection happens per item's alpha mode (over vs additive). Single
//! HDR target with Load.

use super::{draw_item_slot, pipeline_for, PassContext};
use crate::error::RenderResult;
use crate::graph::{MainPass, MATERIAL_TRANSPARENT};

pub(crate) fn run(
    ctx: &mut PassContext,
    encoder: &mut wgpu::CommandEncoder,
) -> RenderResult<()> {
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("anim_render.pass.transparent"),
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
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        let mut last_pipeline: Option<&wgpu::RenderPipeline> = None;
        for (slot, item) in ctx.items.iter().enumerate() {
            if item.material_pass != MATERIAL_TRANSPARENT {
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
    ctx.pass_log.push(MainPass::Transparent);
    Ok(())
}
