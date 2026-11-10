// post.rs -- linear HDR post chain (spec 15.7):
// motion blur > bloom > tone map > grade, in the pinned versioned
// order. The output transfer pass lives in output.rs.
//
// Resource plan (conflict-free ping-pong documented for all four
// enable combos of motion-blur/bloom -- ADR-0010):
//   if motion blur:  motion(scene_hdr + velocity -> P0),
//                    base copy P0 -> scene_hdr
//   if bloom:        bright(scene_hdr -> P1), bloom_v(P1 -> P0),
//                    combine(scene_hdr + P0 -> P1)
//   tone map:        (bloom ? P1 : scene_hdr) -> P0      [exactly once]
//   grade:           P0 -> P1
// The vehicle of the final graded color = post_hdr[1]: the output
// pass reads it (post_hdr[0] when grade is disabled). All targets are
// Rgba16Float (format::POST_HDR); depth is never touched.

use crate::error::RenderResult;
use crate::graph::{MainPass, PostSettings};
use crate::passes::{Pipelines, TargetViews};
use wgpu::CommandEncoder;

/// Raw textures for the one in-chain copy call (views cannot be copied).
pub(crate) struct PostTextures<'a> {
    pub scene_hdr: &'a wgpu::Texture,
    pub post: [&'a wgpu::Texture; 2],
}

/// Per-frame bind groups for the post chain (created by render_scene;
/// they live until the frame's submission, same function scope). The
/// WGSL declares `input_a` at group 1 and `input_b` at group 2 -- each
/// group holds ONE single-texture bind group (tex_layout).
pub(crate) struct PostBinds<'a> {
    // group 0 = params uniform per pass
    pub params_motion: &'a wgpu::BindGroup,
    pub params_bloom_bright: &'a wgpu::BindGroup,
    pub params_bloom_v: &'a wgpu::BindGroup,
    pub params_bloom_combine: &'a wgpu::BindGroup,
    pub params_tone: &'a wgpu::BindGroup,
    pub params_grade: &'a wgpu::BindGroup,
    // texture sets: (group 1, optional group 2)
    pub motion_g1_scene_g2_velocity: (&'a wgpu::BindGroup, &'a wgpu::BindGroup),
    pub bright_g1_scene: &'a wgpu::BindGroup,
    pub bloom_v_g1_post1: &'a wgpu::BindGroup,
    pub combine_g1_scene_g2_post0: (&'a wgpu::BindGroup, &'a wgpu::BindGroup),
    pub tone_g1_scene: &'a wgpu::BindGroup,
    pub tone_g1_post1: &'a wgpu::BindGroup,
    pub grade_g1_post0: &'a wgpu::BindGroup,
}

/// One fullscreen pass: pipeline + params + (1..2) texture groups into
/// a single HDR target. Shared with output.rs.
pub(crate) fn fs_pass(
    encoder: &mut CommandEncoder,
    label: &'static str,
    pipeline: &wgpu::RenderPipeline,
    params_bg: &wgpu::BindGroup,
    g1: &wgpu::BindGroup,
    g2: Option<&wgpu::BindGroup>,
    target: &wgpu::TextureView,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(super::CLEAR_TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, params_bg, &[]);
    pass.set_bind_group(1, g1, &[]);
    if let Some(g2) = g2 {
        pass.set_bind_group(2, g2, &[]);
    }
    pass.draw(0..4, 0..1);
}

/// Execution inputs for the post chain (packed so `run` stays under
/// clippy's argument limit).
pub(crate) struct PostRun<'a> {
    pub pipelines: &'a Pipelines,
    pub settings: &'a PostSettings,
    pub extent: wgpu::Extent3d,
    pub textures: &'a PostTextures<'a>,
    pub binds: PostBinds<'a>,
    pub views: &'a TargetViews,
    pub main_log: &'a mut Vec<MainPass>,
    pub post_log: &'a mut Vec<&'static str>,
}

/// Execute the enabled post chain. Leaves the graded result in
/// post_hdr[1] (post_hdr[0] when grade is disabled). Pushes
/// MainPass::PostChain into main_log once and each executed sub-pass
/// name into post_log (deterministic, G5 readable).
pub(crate) fn run(run: &mut PostRun, encoder: &mut CommandEncoder) -> RenderResult<()> {
    let post = &run.pipelines.post;

    // Motion blur FIRST (the depth-aware slot stays empty until WP07+).
    if run.settings.motion_blur_enabled {
        fs_pass(
            encoder,
            "anim_render.pass.post.motion_blur",
            &post.motion_blur,
            run.binds.params_motion,
            run.binds.motion_g1_scene_g2_velocity.0,
            Some(run.binds.motion_g1_scene_g2_velocity.1),
            &run.views.post[0],
        );
        // The blurred base becomes the scene_hdr input of the bloom /
        // tone-map stages (verified conflict-free ping-pong, ADR-0010).
        encoder.copy_texture_to_texture(
            run.textures.post[0].as_image_copy(),
            run.textures.scene_hdr.as_image_copy(),
            run.extent,
        );
        run.post_log.push("post.motion_blur");
        run.post_log.push("post.base_copy");
    }

    if run.settings.bloom_enabled {
        // Bright+H from the (possibly motion-compensated) scene_hdr.
        fs_pass(
            encoder,
            "anim_render.pass.post.bloom_bright",
            &post.bloom_bright,
            run.binds.params_bloom_bright,
            run.binds.bright_g1_scene,
            None,
            &run.views.post[1],
        );
        // Vertical completion reads exactly what bright wrote (P1).
        fs_pass(
            encoder,
            "anim_render.pass.post.bloom_v",
            &post.bloom_v,
            run.binds.params_bloom_v,
            run.binds.bloom_v_g1_post1,
            None,
            &run.views.post[0],
        );
        // Base (scene_hdr) + bloom (P0) into P1.
        fs_pass(
            encoder,
            "anim_render.pass.post.bloom_combine",
            &post.bloom_combine,
            run.binds.params_bloom_combine,
            run.binds.combine_g1_scene_g2_post0.0,
            Some(run.binds.combine_g1_scene_g2_post0.1),
            &run.views.post[1],
        );
        run.post_log.push("post.bloom_bright");
        run.post_log.push("post.bloom_v");
        run.post_log.push("post.bloom_combine");
    }

    // Tone map: EXACTLY ONCE. Source = combine output when bloom ran,
    // scene_hdr otherwise; destination = P0.
    if run.settings.bloom_enabled {
        fs_pass(
            encoder,
            "anim_render.pass.post.tone_map",
            &post.tone_map,
            run.binds.params_tone,
            run.binds.tone_g1_post1,
            None,
            &run.views.post[0],
        );
    } else {
        fs_pass(
            encoder,
            "anim_render.pass.post.tone_map",
            &post.tone_map,
            run.binds.params_tone,
            run.binds.tone_g1_scene,
            None,
            &run.views.post[0],
        );
    }
    run.post_log.push("post.tone_map");

    // Display-referred grade: P0 into P1.
    if run.settings.grade_enabled {
        fs_pass(
            encoder,
            "anim_render.pass.post.grade",
            &post.grade,
            run.binds.params_grade,
            run.binds.grade_g1_post0,
            None,
            &run.views.post[1],
        );
        run.post_log.push("post.grade");
    }

    run.main_log.push(MainPass::PostChain);
    Ok(())
}
