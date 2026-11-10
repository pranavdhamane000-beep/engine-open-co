//! Opaque (+ alpha-test) forward MRT pass (§15.6).
//!
//! Clears HDR (0,0,0,0), normal (0,0,0,0 — w = validity 0), velocity
//! (0,0,0,0 — z = validity 0) and depth 1.0. Writes depth (Less).
//! Draws every sorted item whose `material_pass == MATERIAL_OPAQUE_MRT`
//! in the stable total order.

use super::{draw_item_slot, pipeline_for, PassContext};
use crate::error::RenderResult;
use crate::graph::{MainPass, MATERIAL_OPAQUE_MRT};

pub(crate) fn run(
    ctx: &mut PassContext,
    encoder: &mut wgpu::CommandEncoder,
) -> RenderResult<()> {
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("anim_render.pass.opaque_mrt"),
            color_attachments: &[
                Some(wgpu::RenderPassColorAttachment {
                    view: &ctx.views.scene_hdr,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(super::CLEAR_TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: &ctx.views.normal,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(super::CLEAR_TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: &ctx.views.velocity,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(super::CLEAR_TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                }),
            ],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &ctx.views.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(crate::targets::format::DEPTH_CLEAR),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        let mut last_pipeline: Option<&wgpu::RenderPipeline> = None;
        for (slot, item) in ctx.items.iter().enumerate() {
            if item.material_pass != MATERIAL_OPAQUE_MRT {
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
    ctx.pass_log.push(MainPass::OpaqueMrt);
    Ok(())
}
