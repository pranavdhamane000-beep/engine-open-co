// ordered_2d.wgsl — explicit 2D layer composition (spec §15.6).
//
// Unlit blended 2D content drawn in PROJECT LAYER ORDER (the CPU's
// stable total sort decides draw order; this pass is painter's
// algorithm). Blended 2D items do NOT write auxiliary normals or
// velocity (documented limitation §15.6). Depth-test (LessEqual, no
// write) against earlier opaque content; equal-depth coplanar 2D ties
// PASS so the draw order (sort) governs.
//
// flags.x alpha kinds:
//   1.0 = straight authored (decode rgb *= a)
//   2.0 = premultipled passthrough (blend One / OneMinusSrcAlpha)

struct CameraBlock2D {
    current: mat4x4<f32>,
};

struct Instance2D {
    current: mat4x4<f32>,
    previous: mat4x4<f32>, // unused here; layout shared with scene.wgsl
    tint: vec4<f32>,
    // x: alpha_kind, y: alpha-test cutoff (unused), z/w: pad
    flags: vec4<f32>,
};

@group(0) @binding(0) var<uniform> cam: CameraBlock2D;
@group(1) @binding(0) var<storage, read> instances: array<Instance2D>;

const CORNERS2D: array<vec4<f32>, 4> = array<vec4<f32>, 4>(
    vec4<f32>(-1.0, -1.0, 0.0, 1.0),
    vec4<f32>(1.0, -1.0, 0.0, 1.0),
    vec4<f32>(-1.0, 1.0, 0.0, 1.0),
    vec4<f32>(1.0, 1.0, 0.0, 1.0),
);

struct VOut2D {
    @builtin(position) clip: vec4<f32>,
    @location(0) tint: vec4<f32>,
    @location(1) flags: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOut2D {
    let inst = instances[ii];
    var out: VOut2D;
    out.clip = cam.current * (inst.current * CORNERS2D[vi]);
    out.tint = inst.tint;
    out.flags = inst.flags;
    return out;
}

struct Frag2D {
    @location(0) color: vec4<f32>,
};

@fragment
fn fs_main(in: VOut2D) -> Frag2D {
    var color = in.tint;
    if (in.flags.x >= 0.5 && in.flags.x < 1.5) {
        // straight-authored: decode to premultiplied
        color = vec4<f32>(color.rgb * color.a, color.a);
    }
    var out: Frag2D;
    out.color = color;
    return out;
}
