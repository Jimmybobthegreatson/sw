// bm-ui builtin 2D shader. Override by shipping `Shaders/WGSL/ui.wgsl` in a mod directory (hot reloaded).
//
// Bindings (keep them when overriding):
//   group(0) binding(0): Screen { size: vec2<f32> }   target size in pixels
//   group(1) binding(0): texture_2d<f32>, binding(1): sampler
// Vertex layout: location 0 pos (pixels, y down), 1 uv, 2 straight-alpha tint (sRGB-authored values).
// Override constant SRGB_TARGET: true when the colour target is an *_SRGB format (the shader then converts
// the authored sRGB values to linear so the hardware encode reproduces them).

override SRGB_TARGET: bool = false;

struct Screen {
    size: vec2<f32>,
    pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> screen: Screen;
@group(1) @binding(0) var ui_tex: texture_2d<f32>;
@group(1) @binding(1) var ui_samp: sampler;

struct VsIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var o: VsOut;
    let ndc = vec2<f32>(v.pos.x / screen.size.x * 2.0 - 1.0, 1.0 - v.pos.y / screen.size.y * 2.0);
    o.clip = vec4<f32>(ndc, 0.0, 1.0);
    o.uv = v.uv;
    o.color = v.color;
    return o;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs_main(i: VsOut) -> @location(0) vec4<f32> {
    let t = textureSample(ui_tex, ui_samp, i.uv);
    var rgb = t.rgb;
    var tint = i.color.rgb;
    if (SRGB_TARGET) {
        rgb = srgb_to_linear(rgb);
        tint = srgb_to_linear(tint);
    }
    return vec4<f32>(rgb * tint, t.a * i.color.a);
}
