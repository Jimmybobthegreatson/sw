// bm-render present pass: display-space scene colour -> render target, with optional FXAA.
//
// Modders: copy this file to `<mod>/Shaders/WGSL/post.wgsl` to replace it (hot reloaded). Entry points `vs_post` / `fs_post`;
// bindings: group 0 = sampler + scene colour texture (display space, Rgba8Unorm).

override TARGET_SRGB: bool = true;  // the target format converts linear -> sRGB on write
override FXAA: bool = true;

@group(0) @binding(0) var samp: sampler;
@group(0) @binding(1) var t_scene: texture_2d<f32>;

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_post(@builtin(vertex_index) i: u32) -> VOut {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: VOut;
    o.uv = vec2<f32>(p.x, 1.0 - p.y);
    o.clip = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    return o;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

fn fetch(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(t_scene, samp, uv, 0.0).rgb;
}

// FXAA 3.11 (quality preset ~12), reimplemented from the public algorithm description.
fn fxaa(uv: vec2<f32>) -> vec3<f32> {
    let dims = vec2<f32>(textureDimensions(t_scene));
    let px = 1.0 / dims;
    let cM = fetch(uv);
    let lM = luma(cM);
    let lN = luma(fetch(uv + vec2<f32>(0.0, -px.y)));
    let lS = luma(fetch(uv + vec2<f32>(0.0, px.y)));
    let lW = luma(fetch(uv + vec2<f32>(-px.x, 0.0)));
    let lE = luma(fetch(uv + vec2<f32>(px.x, 0.0)));
    let lMax = max(lM, max(max(lN, lS), max(lW, lE)));
    let lMin = min(lM, min(min(lN, lS), min(lW, lE)));
    let range = lMax - lMin;
    if (range < max(0.0312, lMax * 0.125)) { return cM; }
    let lNW = luma(fetch(uv + vec2<f32>(-px.x, -px.y)));
    let lNE = luma(fetch(uv + vec2<f32>(px.x, -px.y)));
    let lSW = luma(fetch(uv + vec2<f32>(-px.x, px.y)));
    let lSE = luma(fetch(uv + vec2<f32>(px.x, px.y)));
    let lNS = lN + lS;
    let lWE = lW + lE;
    let edgeH = abs(-2.0 * lW + (lNW + lSW)) + abs(-2.0 * lM + lNS) * 2.0 + abs(-2.0 * lE + (lNE + lSE));
    let edgeV = abs(-2.0 * lN + (lNW + lNE)) + abs(-2.0 * lM + lWE) * 2.0 + abs(-2.0 * lS + (lSW + lSE));
    let horz = edgeH >= edgeV;
    var l1 = select(lW, lN, horz);
    var l2 = select(lE, lS, horz);
    let g1 = abs(l1 - lM);
    let g2 = abs(l2 - lM);
    let steepest = max(g1, g2);
    var stepLen = select(px.x, px.y, horz);
    var lAvg: f32;
    if (g1 >= g2) {
        stepLen = -stepLen;
        lAvg = 0.5 * (l1 + lM);
    } else {
        lAvg = 0.5 * (l2 + lM);
    }
    var cuv = uv;
    if (horz) { cuv.y = cuv.y + stepLen * 0.5; } else { cuv.x = cuv.x + stepLen * 0.5; }
    let off = select(vec2<f32>(px.x, 0.0), vec2<f32>(0.0, px.y), !horz);
    var uv1 = cuv - off;
    var uv2 = cuv + off;
    var e1 = luma(fetch(uv1)) - lAvg;
    var e2 = luma(fetch(uv2)) - lAvg;
    let thr = steepest * 0.25;
    var done1 = abs(e1) >= thr;
    var done2 = abs(e2) >= thr;
    for (var i = 0; i < 10; i = i + 1) {
        if (done1 && done2) { break; }
        if (!done1) { uv1 = uv1 - off; e1 = luma(fetch(uv1)) - lAvg; done1 = abs(e1) >= thr; }
        if (!done2) { uv2 = uv2 + off; e2 = luma(fetch(uv2)) - lAvg; done2 = abs(e2) >= thr; }
    }
    var d1 = select(uv.y - uv1.y, uv.x - uv1.x, horz);
    var d2 = select(uv2.y - uv.y, uv2.x - uv.x, horz);
    let nearer1 = d1 < d2;
    let dist = min(d1, d2);
    let span = d1 + d2;
    let centerSmaller = lM < lAvg;
    let endSmaller = select(e2 < 0.0, e1 < 0.0, nearer1);
    var pixelOffset = -dist / span + 0.5;
    if (centerSmaller == endSmaller) { pixelOffset = 0.0; }
    let lAvg3 = (2.0 * (lNS + lWE) + (lNW + lNE + lSW + lSE)) / 12.0;
    let sub = clamp(abs(lAvg3 - lM) / range, 0.0, 1.0);
    let subOffset = (-2.0 * sub + 3.0) * sub * sub;
    let finalOffset = max(pixelOffset, subOffset * subOffset * 0.75);
    var fuv = uv;
    if (horz) { fuv.y = fuv.y + finalOffset * stepLen; } else { fuv.x = fuv.x + finalOffset * stepLen; }
    return fetch(fuv);
}

@fragment
fn fs_post(in: VOut) -> @location(0) vec4<f32> {
    var c: vec3<f32>;
    if (FXAA) { c = fxaa(in.uv); } else { c = fetch(in.uv); }
    if (TARGET_SRGB) { c = srgb_to_linear(c); }
    return vec4<f32>(c, 1.0);
}
