// output.wgsl — output transfer, alpha handling and export output
// (spec §15.7 Post graph, §15.3 files list).
//
// "Tone mapping runs once. Display-referred grading and output transfer
// follow their declared profile." This pass is the LAST of the frame:
// input = final display-referred graded color; output = the configured
// surface/export target.
//
// Transfer-function contract (G5 "exactly one output transfer"):
//   params.p0 == 0.0 : target format IS sRGB-aware (Rgba8UnormSrgb, …)
//                      → the shader emits LINEAR and the hardware
//                      performs the encode exactly once.
//   params.p0 == 1.0 : target format is linearly-encoded (Rgba8Unorm, …)
//                      → the shader performs the manual linear→sRGB
//                      encode exactly once.
//
// Premultiplied → straight conversion happens ONLY when the selected
// codec requires it (anim_export, WP09) — this pass never silently
// converts. Zero-alpha RGB is explicitly defined as [0, 0, 0].

struct OutputParams {
    transfer_manual: f32,
    pad_y: f32,
    pad_z: f32,
    pad_w: f32,
};

@group(0) @binding(0) var<uniform> params: OutputParams;
@group(1) @binding(0) var input_a: texture_2d<f32>;

const OUT_CORNERS: array<vec4<f32>, 4> = array<vec4<f32>, 4>(
    vec4<f32>(-1.0, -1.0, 0.0, 1.0),
    vec4<f32>(1.0, -1.0, 0.0, 1.0),
    vec4<f32>(-1.0, 1.0, 0.0, 1.0),
    vec4<f32>(1.0, 1.0, 0.0, 1.0),
);

struct OutVOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

struct OutColor {
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> OutVOut {
    var out: OutVOut;
    out.clip = OUT_CORNERS[vi];
    out.uv = OUT_CORNERS[vi].xy * 0.5 + vec2<f32>(0.5, 0.5);
    return out;
}

fn out_pixel_of(uv: vec2<f32>, size: vec2<u32>) -> vec2<i32> {
    let raw = vec2<f32>(f32(size.x), f32(size.y)) * uv - vec2<f32>(0.5, 0.5);
    return clamp(
        vec2<i32>(round(raw)),
        vec2<i32>(0, 0),
        vec2<i32>(i32(size.x) - 1, i32(size.y) - 1),
    );
}

@fragment
fn fs_main(in: OutVOut) -> OutColor {
    let size = textureDimensions(input_a);
    let px = out_pixel_of(in.uv, size);
    let c = textureLoad(input_a, px, 0);
    let lin = clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    var out: OutColor;
    if (params.transfer_manual > 0.5) {
        // Manual linear ----------> sRGB ENCODE (piecewise IEC 61966-2-1).
        // The decode direction pow((x+0.055)/1.055, 2.4) was here before
        // and produced byte-23 style garbage — caught by the G5 exactly-
        // once golden (ADR-0010).
        let srgb = select(
            lin * 12.92,
            1.055 * pow(lin, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055),
            vec3<bool>(lin > vec3<f32>(0.0031308)),
        );
        out.color = vec4<f32>(srgb, c.a);
    } else {
        out.color = vec4<f32>(lin, c.a);
    }
    return out;
}
