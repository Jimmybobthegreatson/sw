// Builtin particle shader of bm-particles. Override by shipping `Shaders/WGSL/particles.wgsl` in a mod
// (copy this file as a template). Keep the bindings and the entry points `vs_main` / `fs_main`.
//
// group 0: frame uniform            group 1: sprite texture + sampler
// per instance (vertex buffer slot 0, 6 vertices per instance):
//   center, half-extent axes x/y (already camera facing / stretched on the CPU), straight-alpha colour, uv rect.

override GAMMA_ENCODE: bool = false;

struct Frame {
    view_proj: mat4x4<f32>,
    // x: global opacity multiplier, y: colour exponent (2.2 = authored values are sRGB), z,w: reserved
    params: vec4<f32>,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var sprite_tex: texture_2d<f32>;
@group(1) @binding(1) var sprite_samp: sampler;

struct VsIn {
    @builtin(vertex_index) vi: u32,
    @location(0) center: vec3<f32>,
    @location(1) axis_x: vec3<f32>,
    @location(2) axis_y: vec3<f32>,
    @location(3) color: vec4<f32>,
    @location(4) uv: vec4<f32>,
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let c = corners[in.vi];
    let world = in.center + in.axis_x * c.x + in.axis_y * c.y;
    var o: VsOut;
    o.clip = frame.view_proj * vec4<f32>(world, 1.0);
    o.uv = vec2<f32>(mix(in.uv.x, in.uv.z, c.x * 0.5 + 0.5), mix(in.uv.w, in.uv.y, c.y * 0.5 + 0.5));
    o.color = in.color;
    return o;
}

fn encode(c: vec3<f32>) -> vec3<f32> {
    return select(c, pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2)), GAMMA_ENCODE);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(sprite_tex, sprite_samp, in.uv);       // sRGB texture: sampled as linear
    let tint = pow(max(in.color.rgb, vec3<f32>(0.0)), vec3<f32>(frame.params.y));
    let a = texel.a * in.color.a * frame.params.x;
    if (a <= 0.002) {
        discard;
    }
    return vec4<f32>(encode(texel.rgb * tint), a);
}
