// scene.wgsl — Opaque/alpha-test forward MRT pass (spec §15.6, §15.7).
//
// Outputs:
//   0: linear scene-referred HDR color (premultiplied)
//   1: view-space unit normal in xyz (sprites: +z plane), validity in w
//   2: screen-space velocity: (current_uv - previous_uv) in xy,
//      validity flag in z, reactive-mask reserved in w (0.0)
//
// Velocity math (pinned §15.7):
//   uv = ndc_xy * vec2(0.5, -0.5) + 0.5
//   current clip = current_vp * (item_current * corner)
//   previous clip = previous_vp * (item_previous * corner)
//   invalid when: per-item invalid flag, previous w <= 0.
//
// Alpha-test semantics: discard BELOW cutoff BEFORE any attachment
// write. `flags.x`: 0.0 = alpha-test path (cutoff in flags.y), 3.0 =
// straight decode (rgb *= a) for crafted opaque-ish items.

struct CameraBlock {
    current: mat4x4<f32>,
    previous: mat4x4<f32>,
};

struct Instance {
    current: mat4x4<f32>,
    previous: mat4x4<f32>,
    tint: vec4<f32>,
    // x: alpha_kind, y: alpha-test cutoff, z: velocity_valid, w: pad
    flags: vec4<f32>,
};

@group(0) @binding(0) var<uniform> cam: CameraBlock;
@group(1) @binding(0) var<storage, read> instances: array<Instance>;

const CORNERS: array<vec4<f32>, 4> = array<vec4<f32>, 4>(
    vec4<f32>(-1.0, -1.0, 0.0, 1.0),
    vec4<f32>(1.0, -1.0, 0.0, 1.0),
    vec4<f32>(-1.0, 1.0, 0.0, 1.0),
    vec4<f32>(1.0, 1.0, 0.0, 1.0),
);

fn ndc_to_uv(ndc: vec2<f32>) -> vec2<f32> {
    return ndc * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
}

struct VOut {
    @builtin(position) clip: vec4<f32>,
    // Plain interpolants (NOT the builtin position, whose xy is already
    // converted to PIXEL space by the rasterizer): raw clip values
    // interpolated so the fragment can reconstruct NDC by /w.
    @location(0) cur_clip: vec4<f32>,
    @location(1) prev_clip: vec4<f32>,
    @location(2) tint: vec4<f32>,
    @location(3) flags: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOut {
    let inst = instances[ii];
    let world = inst.current * CORNERS[vi];
    let prev_world = inst.previous * CORNERS[vi];
    var out: VOut;
    let cur = cam.current * world;
    out.clip = cur;
    out.cur_clip = cur;
    out.prev_clip = cam.previous * prev_world;
    out.tint = inst.tint;
    out.flags = inst.flags;
    return out;
}

struct MRT {
    @location(0) color: vec4<f32>,
    @location(1) normal: vec4<f32>,
    @location(2) velocity: vec4<f32>,
};

@fragment
fn fs_main(in: VOut) -> MRT {
    // Alpha test BEFORE any attachment write (§15.7).
    let is_alpha_test = in.flags.x < 0.5;
    if (is_alpha_test && in.flags.y > 0.0 && in.tint.a < in.flags.y) {
        discard;
    }

    // MVP opaque material: straight-authored tint decoded to premul.
    var color = in.tint;
    if (!is_alpha_test && in.flags.x >= 2.5 && in.flags.x < 3.5) {
        color = vec4<f32>(color.rgb * color.a, color.a);
    }

    // Sprites face the camera plane: view-space normal +z, valid.
    let normal = vec4<f32>(0.0, 0.0, 1.0, 1.0);

    // Screen-space velocity (unjittered; jitter is a future WP07+ path).
    // uv_now comes from the PLAIN cur_clip interpolant — the
    // @builtin(position) xy is already pixel space and would produce
    // garbage deltas.
    let w_now = in.cur_clip.w;
    let uv_now = ndc_to_uv(in.cur_clip.xy / max(w_now, 1e-6));
    var velocity = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    if (in.flags.z > 0.5 && w_now > 0.0 && in.prev_clip.w > 0.0) {
        let uv_prev = ndc_to_uv(in.prev_clip.xy / in.prev_clip.w);
        velocity = vec4<f32>(uv_now - uv_prev, 1.0, 0.0);
    }

    var out: MRT;
    out.color = color;
    out.normal = normal;
    out.velocity = velocity;
    return out;
}
