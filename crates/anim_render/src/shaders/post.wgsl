// post.wgsl — linear HDR post chain and tone mapping (spec §15.7).
//
// HDR post effects operate BEFORE tone mapping. Tone mapping runs
// EXACTLY ONCE (chain planner enforces it — graph.rs).
//
// Entry points (one pass each, ping-pong targets):
//   fs_motion_blur    — reads scene HDR + velocity; average along
//                       screen-space velocity, clamped to target bounds
//   fs_bloom_bright   — bright-pass (HDR threshold) + horizontal tap
//                       blend, single fixed-radius pass (documented
//                       MVP "bloom-lite"; full separable gaussian is a
//                       declared WP07+ refinement)
//   fs_bloom_v        — vertical tap completion of the bloom layer
//   fs_bloom_combine  — base + bloom (linear add, still HDR)
//   fs_tone_map       — Reinhard c / (1 + c), pinned curve (ADR-0010)
//   fs_grade          — display-referred clamp into [0,1] at neutral
//                       exposure (identity grading at defaults)
//   fs_transfer       — output transfer: passthrough when the target
//                       format is sRGB-capable (hardware encodes exactly
//                       once), manual linear→sRGB otherwise.
//
// All sampling is textureLoad on exact pixel coordinates — no filter
// error; each pass is reproducible for the G5 goldens.

struct PostParams {
    p0: f32, // step-specific parameter
    p1: f32,
    p2: f32,
    p3: f32,
};

@group(0) @binding(0) var<uniform> params: PostParams;
@group(1) @binding(0) var input_a: texture_2d<f32>;
@group(2) @binding(0) var input_b: texture_2d<f32>;

const FS_CORNERS: array<vec4<f32>, 4> = array<vec4<f32>, 4>(
    vec4<f32>(-1.0, -1.0, 0.0, 1.0),
    vec4<f32>(1.0, -1.0, 0.0, 1.0),
    vec4<f32>(-1.0, 1.0, 0.0, 1.0),
    vec4<f32>(1.0, 1.0, 0.0, 1.0),
);

struct FsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

struct FsColor {
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_fullscreen(@builtin(vertex_index) vi: u32) -> FsOut {
    var out: FsOut;
    out.clip = FS_CORNERS[vi];
    out.uv = FS_CORNERS[vi].xy * 0.5 + vec2<f32>(0.5, 0.5);
    return out;
}

fn pixel_of(uv: vec2<f32>, size: vec2<u32>) -> vec2<i32> {
    let raw = vec2<f32>(f32(size.x), f32(size.y)) * uv - vec2<f32>(0.5, 0.5);
    return clamp(
        vec2<i32>(round(raw)),
        vec2<i32>(0, 0),
        vec2<i32>(i32(size.x) - 1, i32(size.y) - 1),
    );
}

/// Luminance weights pinned here (Rec. 709); matches the CPU reference.
const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

fn bright_extract(c: vec4<f32>, threshold: f32, gain: f32) -> vec3<f32> {
    let lum = dot(c.rgb, LUMA);
    return select(vec3<f32>(0.0), c.rgb * gain, vec3<bool>(lum > threshold));
}

@fragment
fn fs_motion_blur(in: FsOut) -> FsColor {
    let size = textureDimensions(input_a);
    let px = pixel_of(in.uv, size);
    let vd = textureLoad(input_b, px, 0);
    let valid = vd.z > 0.5;
    let steps = i32(params.p0);
    var acc = textureLoad(input_a, px, 0);
    if (valid && steps > 1) {
        var sum = acc.rgb;
        let size_f = vec2<f32>(f32(size.x), f32(size.y));
        for (var s: i32 = 1; s < steps; s = s + 1) {
            let t = f32(s) / f32(steps - 1);
            let offset_f = vec2<f32>(f32(px.x), f32(px.y)) - vd.xy * size_f * t;
            let sample_px = vec2<i32>(
                i32(clamp(offset_f.x, 0.0, f32(size.x) - 1.0)),
                i32(clamp(offset_f.y, 0.0, f32(size.y) - 1.0)),
            );
            sum = sum + textureLoad(input_a, sample_px, 0).rgb;
        }
        acc = vec4<f32>(sum / f32(steps), acc.a);
    }
    var out: FsColor;
    out.color = acc;
    return out;
}

@fragment
fn fs_bloom_bright(in: FsOut) -> FsColor {
    let size = textureDimensions(input_a);
    let px = pixel_of(in.uv, size);
    let radius = i32(params.p2);
    let denominator = f32(radius * 2 + 1);
    var sum = vec3<f32>(0.0);
    for (var d: i32 = -radius; d <= radius; d = d + 1) {
        let npx = vec2<i32>(clamp(px.x + d, 0, i32(size.x) - 1), px.y);
        sum = sum + bright_extract(textureLoad(input_a, npx, 0), params.p0, params.p1);
    }
    var out: FsColor;
    out.color = vec4<f32>(sum / denominator, 0.0);
    return out;
}

@fragment
fn fs_bloom_v(in: FsOut) -> FsColor {
    let size = textureDimensions(input_a);
    let px = pixel_of(in.uv, size);
    let radius = i32(params.p2);
    let denominator = f32(radius * 2 + 1);
    var sum = vec3<f32>(0.0);
    for (var d: i32 = -radius; d <= radius; d = d + 1) {
        let npx = vec2<i32>(px.x, clamp(px.y + d, 0, i32(size.y) - 1));
        sum = sum + textureLoad(input_a, npx, 0).rgb;
    }
    var out: FsColor;
    out.color = vec4<f32>(sum / denominator, 0.0);
    return out;
}

@fragment
fn fs_bloom_combine(in: FsOut) -> FsColor {
    let size = textureDimensions(input_a);
    let px = pixel_of(in.uv, size);
    let base = textureLoad(input_a, px, 0);
    let bloom = textureLoad(input_b, px, 0);
    var out: FsColor;
    out.color = vec4<f32>(base.rgb + bloom.rgb * params.p3, base.a);
    return out;
}

@fragment
fn fs_tone_map(in: FsOut) -> FsColor {
    let size = textureDimensions(input_a);
    let px = pixel_of(in.uv, size);
    let c = textureLoad(input_a, px, 0);
    // Reinhard: c / (1 + c) — pinned for WP06 (ADR-0010).
    var out: FsColor;
    out.color = vec4<f32>(c.rgb / (1.0 + c.rgb), c.a);
    return out;
}

@fragment
fn fs_grade(in: FsOut) -> FsColor {
    let size = textureDimensions(input_a);
    let px = pixel_of(in.uv, size);
    let c = textureLoad(input_a, px, 0);
    // Neutral display-referred grade = clamp into display range.
    // (Exposure/contrast curves land with the grading profile, WP08/09.)
    var out: FsColor;
    out.color = vec4<f32>(clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), c.a);
    return out;
}
