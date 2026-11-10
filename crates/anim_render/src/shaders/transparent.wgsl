// transparent.wgsl — sorted transparent forward pass (spec §15.6, §15.7).
//
// Depth-TEST without depth writes; NO normals/velocity writes in the
// initial material contract (limitation documented, §15.6). Blending is
// pipeline-selected:
//   over  : One / OneMinusSrcAlpha  (alpha kinds 1 straight, 2 premul)
//   additive: One / One             (alpha kind 3; rgb carries coverage)

struct CameraBlockT {
    current: mat4x4<f32>,
};

struct InstanceT {
    current: mat4x4<f32>,
    previous: mat4x4<f32>,
    tint: vec4<f32>,
    // x: alpha_kind, y/z/w: unused here
    flags: vec4<f32>,
};

@group(0) @binding(0) var<uniform> cam: CameraBlockT;
@group(1) @binding(0) var<storage, read> instances: array<InstanceT>;

const CORNERS_T: array<vec4<f32>, 4> = array<vec4<f32>, 4>(
    vec4<f32>(-1.0, -1.0, 0.0, 1.0),
    vec4<f32>(1.0, -1.0, 0.0, 1.0),
    vec4<f32>(-1.0, 1.0, 0.0, 1.0),
    vec4<f32>(1.0, 1.0, 0.0, 1.0),
);

struct VOutT {
    @builtin(position) clip: vec4<f32>,
    @location(0) tint: vec4<f32>,
    @location(1) flags: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOutT {
    let inst = instances[ii];
    var out: VOutT;
    out.clip = cam.current * (inst.current * CORNERS_T[vi]);
    out.tint = inst.tint;
    out.flags = inst.flags;
    return out;
}

struct FragT {
    @location(0) color: vec4<f32>,
};

@fragment
fn fs_main(in: VOutT) -> FragT {
    var color = in.tint;
    if (in.flags.x >= 0.5 && in.flags.x < 1.5) {
        // straight-authored transparent: decode to premultiplied
        color = vec4<f32>(color.rgb * color.a, color.a);
    }
    // premul (2) and additive (3) pass through unchanged
    var out: FragT;
    out.color = color;
    return out;
}
