// bm-render builtin uber shader (forward lighting driven by the .MAT parameters and the decoded scene lighting).
//
// Modders: copy this file to `<mod>/Shaders/WGSL/main.wgsl` to replace it globally (hot reloaded), or
// write a separate module that keeps the same bindings / entry points (`vs_main`, `fs_main`) and select
// it per material through `RenderHooks::material` -> `MaterialOverride::shader`.
//
// Native space: left-handed, Y-up, centimetres. Front faces are counter-clockwise on screen (the pipeline uses FrontFace::Ccw).
// The projection is reverse-Z (depth 1 = near, 0 = far); all values reaching the shader are plain world space.
//
// COLOUR SPACE: like the original engine everything is computed in *display* (gamma) space: textures are uploaded as plain
// `Rgba8Unorm`, light / ambient / fog colours are the display-space values from the data, and the result is written to an
// `Rgba8Unorm` scene target that the present pass (`post.wgsl`) converts to the swap chain format.

override SKINNED: bool = false;
override ALPHA_TEST: bool = false;
override SKY_LAYER: bool = false; // sky dome / backdrop items: unlit, no fog, camera centred, no depth

struct Light {
    pos_range: vec4<f32>,  // xyz position (cm), w range end (cm)
    color_inv: vec4<f32>,  // rgb radiance, w 1 / (range end - range start)
    dir_cos: vec4<f32>,    // xyz spot axis (direction of travel), w cos(outer)
    spot: vec4<f32>,       // x 1 = spot light, y 1 / (cos inner - cos outer)
};

struct Frame {
    view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,        // xyz, w = time (s)
    cam_right: vec4<f32>,      // right * tan(fovx/2)
    cam_up: vec4<f32>,         // up * tan(fovy/2)
    cam_fwd: vec4<f32>,
    sun_dir: vec4<f32>,        // xyz = unit vector TOWARDS the sun, w = debug mode
    sun_color: vec4<f32>,      // rgb * intensity, w = exposure
    ambient_sky: vec4<f32>,    // rgb, w = reflection strength
    ambient_ground: vec4<f32>,
    fog_color: vec4<f32>,      // rgb, w = fog end (cm)
    fog_params: vec4<f32>,     // x = fog start (cm), y = vertex colour strength, z = light count
    sky_top: vec4<f32>,         // rgb, w = sky dome gain
    sky_horizon: vec4<f32>,
    shadow_vp: array<mat4x4<f32>, 3>,
    shadow_splits: vec4<f32>,  // x, y, z = far view depth of cascade 0, 1, 2 (cm), w = cascade count (0 = off)
    shadow_params: vec4<f32>,  // x = strength, y = depth bias (texels), z = normal bias (texels), w = 1 / resolution
    shadow_texel: vec4<f32>,   // world size of one shadow texel per cascade (cm)
    shadow_range: vec4<f32>,   // depth range (cm) of each cascade
    lights: array<Light, 16>,
};

@group(0) @binding(0) var<uniform> u_frame: Frame;
@group(0) @binding(1) var<storage, read> bones: array<mat4x4<f32>>;
@group(0) @binding(2) var t_shadow: texture_depth_2d_array;
@group(0) @binding(3) var s_shadow: sampler_comparison;

struct Material {
    diffuse: vec4<f32>,   // v4DiffuseColor
    specular: vec4<f32>,  // rgb v4SpecularColor, w specular level
    illum: vec4<f32>,     // rgb v4IlluminationColor
    params: vec4<f32>,    // x specular power, y alpha ref (0..255), z opacity, w reflection level
    refl: vec4<f32>,      // rgb v4ReflectionColor
    fade: vec4<f32>,      // alpha fade: x angle scale, y angle offset, z distance scale, w distance offset
    flags: vec4<u32>,     // x bit field, see F_*
};

const F_NORMAL: u32 = 1u;
const F_SPEC: u32 = 2u;
const F_ILLUM: u32 = 4u;
const F_TWO_SIDED: u32 = 8u;
const F_FOG: u32 = 16u;
const F_UNLIT: u32 = 32u;
const F_SPEC_ENABLED: u32 = 64u;
const F_ALPHA_FADE: u32 = 128u;
const F_REFLECT_MAP: u32 = 256u;   // mapEnvironment present (sphere map)
const F_REFLECT_SKY: u32 = 512u;   // reflection enabled without a map: procedural sky
const F_REFL_MASK: u32 = 1024u;
const F_REFL_FALLOFF: u32 = 2048u;
const F_GLASS: u32 = 8192u;        // blended window pane: opacity = 1 - 0.6 * reflection mask
const F_WATER: u32 = 4096u;        // WaterSurface: fade.xyz = fresnel min / max / power, fade.w = bump scale

@group(1) @binding(0) var<uniform> u_mat: Material;
@group(1) @binding(1) var samp: sampler;
@group(1) @binding(2) var t_diffuse: texture_2d<f32>;
@group(1) @binding(3) var t_normal: texture_2d<f32>;
@group(1) @binding(4) var t_spec: texture_2d<f32>;
@group(1) @binding(5) var t_illum: texture_2d<f32>;
@group(1) @binding(6) var t_env: texture_2d<f32>;
@group(1) @binding(7) var t_rmask: texture_2d<f32>;
@group(1) @binding(8) var t_falloff: texture_2d<f32>;

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent: vec4<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) color: vec4<f32>,
    @location(5) joints: vec4<u32>,
    @location(6) weights: vec4<f32>,
    @location(8) m0: vec4<f32>,
    @location(9) m1: vec4<f32>,
    @location(10) m2: vec4<f32>,
    @location(11) m3: vec4<f32>,
    @location(12) bone_base: u32,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent: vec4<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) color: vec4<f32>,
};

@vertex
fn vs_main(v: VIn) -> VOut {
    let model = mat4x4<f32>(v.m0, v.m1, v.m2, v.m3);
    var p = vec4<f32>(v.pos, 1.0);
    var n = v.normal;
    var t = v.tangent.xyz;
    if (SKINNED) {
        let b = v.bone_base;
        let m = bones[b + v.joints.x] * v.weights.x + bones[b + v.joints.y] * v.weights.y
              + bones[b + v.joints.z] * v.weights.z + bones[b + v.joints.w] * v.weights.w;
        p = m * p;
        n = (m * vec4<f32>(n, 0.0)).xyz;
        t = (m * vec4<f32>(t, 0.0)).xyz;
    }
    let w = model * p;
    var o: VOut;
    o.world = w.xyz;
    o.clip = u_frame.view_proj * w;
    o.normal = (model * vec4<f32>(n, 0.0)).xyz;
    o.tangent = vec4<f32>((model * vec4<f32>(t, 0.0)).xyz, v.tangent.w);
    o.uv = v.uv;
    o.color = v.color;
    return o;
}

// ---- shadow pass --------------------------------------------------------------------------------------------
// `u_frame.view_proj` holds the light's orthographic matrix in the shadow passes.

struct ShadowOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_shadow(v: VIn) -> ShadowOut {
    let model = mat4x4<f32>(v.m0, v.m1, v.m2, v.m3);
    var p = vec4<f32>(v.pos, 1.0);
    if (SKINNED) {
        let b = v.bone_base;
        let m = bones[b + v.joints.x] * v.weights.x + bones[b + v.joints.y] * v.weights.y
              + bones[b + v.joints.z] * v.weights.z + bones[b + v.joints.w] * v.weights.w;
        p = m * p;
    }
    var o: ShadowOut;
    o.clip = u_frame.view_proj * (model * p);
    o.uv = v.uv;
    return o;
}

// only used by the alpha tested variant (foliage, fences)
@fragment
fn fs_shadow(in: ShadowOut) {
    let a = textureSample(t_diffuse, samp, in.uv).a * u_mat.diffuse.a * u_mat.params.z;
    if (a * 255.0 < max(u_mat.params.y, 128.0)) { discard; }
}

fn shadow_factor(world: vec3<f32>, n: vec3<f32>, ndl: f32) -> f32 {
    let count = u32(u_frame.shadow_splits.w);
    if (count == 0u) { return 1.0; }
    let vd = dot(world - u_frame.cam_pos.xyz, u_frame.cam_fwd.xyz);
    var ci = 0u;
    if (count > 1u && vd > u_frame.shadow_splits.x) { ci = 1u; }
    if (count > 2u && vd > u_frame.shadow_splits.y) { ci = 2u; }
    let far = u_frame.shadow_splits[ci];
    if (vd > far) { return 1.0; }
    let texel = u_frame.shadow_texel[ci];
    let slope = clamp(1.0 - ndl, 0.0, 1.0);
    let wp = world + n * (u_frame.shadow_params.z * texel * (0.5 + slope));
    let lp = u_frame.shadow_vp[ci] * vec4<f32>(wp, 1.0);
    let uv = vec2<f32>(lp.x * 0.5 + 0.5, -lp.y * 0.5 + 0.5);
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || lp.z > 1.0) { return 1.0; }
    let bias = u_frame.shadow_params.y * texel * (1.0 + slope * 2.0) / u_frame.shadow_range[ci];
    let z = lp.z - bias;
    let ts = u_frame.shadow_params.w;
    var sum = 0.0;
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            let o = vec2<f32>(f32(x), f32(y)) * ts;
            sum += textureSampleCompareLevel(t_shadow, s_shadow, uv + o, i32(ci), z);
        }
    }
    var lit = sum / 9.0;
    // fade out at the end of the last cascade
    let fade = clamp((far - vd) / (far * 0.15), 0.0, 1.0);
    lit = mix(1.0, lit, fade);
    return lit;
}

fn sky_color(dir: vec3<f32>) -> vec3<f32> {
    let t = clamp(dir.y, 0.0, 1.0);
    var col = mix(u_frame.sky_horizon.rgb, u_frame.sky_top.rgb, pow(t, 0.55));
    if (dir.y < 0.0) {
        col = mix(u_frame.sky_horizon.rgb, u_frame.ambient_ground.rgb, clamp(-dir.y * 4.0, 0.0, 1.0));
    }
    let s = max(dot(dir, u_frame.sun_dir.xyz), 0.0);
    col += u_frame.sun_color.rgb * (pow(s, 600.0) * 4.0 + pow(s, 12.0) * 0.12);
    return col;
}

@fragment
fn fs_main(in: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let flags = u_mat.flags.x;
    var uv = in.uv;
    if ((flags & F_WATER) != 0u) {
        // scrolling ripples
        uv = in.uv + vec2<f32>(u_frame.cam_pos.w * 0.006, u_frame.cam_pos.w * 0.004);
    }
    let tex = textureSample(t_diffuse, samp, in.uv);
    let nmap = textureSample(t_normal, samp, uv);
    let smap = textureSample(t_spec, samp, in.uv);
    let imap = textureSample(t_illum, samp, in.uv);

    let base = tex * u_mat.diffuse;
    // alpha = texture alpha x material opacity x vertex alpha (the `*_Opac` ground / decal / blob shadow meshes fade through it)
    var alpha = base.a * u_mat.params.z * in.color.a;
    if ((flags & F_GLASS) != 0u) { alpha = alpha * (1.0 - 0.6 * textureSample(t_rmask, samp, in.uv).r); }
    let to_cam = u_frame.cam_pos.xyz - in.world;
    let dist = length(to_cam);
    let V = to_cam / max(dist, 1e-4);

    var n = normalize(in.normal);
    if (!front && (flags & F_TWO_SIDED) != 0u) { n = -n; }
    let geo_n = n;
    if ((flags & F_ALPHA_FADE) != 0u) {
        // "falloff" glow cards: fade with the viewing angle and the distance (best guess of the parameter meaning)
        let a1 = clamp(abs(dot(geo_n, V)) * u_mat.fade.x + u_mat.fade.y, 0.0, 1.0);
        let a2 = clamp(dist * u_mat.fade.z + u_mat.fade.w, 0.0, 1.0);
        alpha = alpha * a1 * a2;
    }
    if (ALPHA_TEST) {
        if (alpha * 255.0 < u_mat.params.y) { discard; }
    }
    if ((flags & F_NORMAL) != 0u && in.tangent.w != 0.0) {
        var xy = nmap.xy * 2.0 - 1.0;
        if ((flags & F_WATER) != 0u) { xy = xy * clamp(u_mat.fade.w * 2.0, 0.1, 1.0); }
        let z = sqrt(max(0.0, 1.0 - dot(xy, xy)));
        let tn = normalize(in.tangent.xyz - n * dot(n, in.tangent.xyz));
        let bt = cross(n, tn) * in.tangent.w;
        n = normalize(tn * xy.x + bt * xy.y + n * z);
    }

    let vc = mix(vec3<f32>(1.0), in.color.rgb, u_frame.fog_params.y);
    if (SKY_LAYER && (flags & F_WATER) == 0u) {
        var sc = base.rgb * in.color.rgb * (u_frame.sky_top.w * u_frame.sun_color.w);
        if ((flags & F_UNLIT) == 0u) {
            // The dome ends in a pale disc just below the horizon where the terrain stops; in the engine the distance fog colour
            // (CameraBackgroundColor) covers it. Blend the opaque dome into the fog colour towards / below the horizon so that the
            // far terrain, the disc and the sky form one continuous haze instead of a hard pale band.
            let dv = normalize(in.world - u_frame.cam_pos.xyz);
            let haze = 1.0 - smoothstep(0.0, 0.22, dv.y);
            sc = mix(sc, u_frame.fog_color.rgb * u_frame.sun_color.w, haze * 0.9);
        }
        return vec4<f32>(sc, alpha);
    }
    let mode = u32(u_frame.sun_dir.w);
    if (mode == 1u) { return vec4<f32>(base.rgb, 1.0); }
    if (mode == 2u) { return vec4<f32>(n * 0.5 + 0.5, 1.0); }

    var col: vec3<f32>;
    var refl_lum = 0.0;
    if ((flags & F_WATER) != 0u) {
        let ndv = clamp(dot(n, V), 0.0, 1.0);
        let fres = clamp(u_mat.fade.x + (u_mat.fade.y - u_mat.fade.x) * pow(1.0 - ndv, max(u_mat.fade.z, 1.0)), 0.0, 1.0);
        let L = u_frame.sun_dir.xyz;
        let deep = vec3<f32>(0.05, 0.13, 0.17) * (u_frame.ambient_sky.rgb * 1.5 + u_frame.sun_color.rgb * 0.35 + vec3<f32>(0.1));
        let R = reflect(-V, n);
        let sky = sky_color(R);
        col = mix(deep, sky, fres);
        let H = normalize(L + V);
        col += u_mat.specular.rgb * u_mat.specular.w * 0.25 * pow(max(dot(n, H), 0.0), max(u_mat.params.x, 1.0)) * u_frame.sun_color.rgb;
        col = col * u_frame.sun_color.w;
    } else if ((flags & F_UNLIT) != 0u) {
        col = base.rgb * vc * u_frame.sun_color.w;
    } else {
        let L = u_frame.sun_dir.xyz;
        let ndl = max(dot(n, L), 0.0);
        let hemi = mix(u_frame.ambient_ground.rgb, u_frame.ambient_sky.rgb, n.y * 0.5 + 0.5);
        var sh = 1.0;
        if (ndl > 0.0 && u_frame.shadow_splits.w > 0.5) {
            sh = mix(1.0, shadow_factor(in.world, geo_n, max(dot(geo_n, L), 0.0)), u_frame.shadow_params.x);
        }
        if (mode == 3u) { return vec4<f32>(vec3<f32>(sh), 1.0); }
        var diff = hemi + u_frame.sun_color.rgb * (ndl * sh);
        var spec = vec3<f32>(0.0);
        let smask = smap.r;
        let spow = max(u_mat.params.x, 1.0);
        let scol = u_mat.specular.rgb * u_mat.specular.w;
        let enable_spec = (flags & F_SPEC_ENABLED) != 0u;
        let sm = select(1.0, smask, (flags & F_SPEC) != 0u);
        if (enable_spec) {
            let H = normalize(L + V);
            spec += scol * sm * pow(max(dot(n, H), 0.0), spow) * u_frame.sun_color.rgb * (sh * step(0.0, ndl));
        }
        let count = u32(u_frame.fog_params.z);
        for (var i = 0u; i < count; i = i + 1u) {
            let li = u_frame.lights[i];
            let d = li.pos_range.xyz - in.world;
            let dl = length(d);
            var att = clamp((li.pos_range.w - dl) * li.color_inv.w, 0.0, 1.0);
            let Lp = d / max(dl, 1e-4);
            if (li.spot.x > 0.5) {
                let cs = dot(-Lp, li.dir_cos.xyz);
                att = att * clamp((cs - li.dir_cos.w) * li.spot.y, 0.0, 1.0);
            }
            let nl = max(dot(n, Lp), 0.0);
            diff += li.color_inv.rgb * (att * nl);
            if (enable_spec) {
                let H = normalize(Lp + V);
                spec += scol * sm * pow(max(dot(n, H), 0.0), spow) * li.color_inv.rgb * (att * step(0.0, nl));
            }
        }
        col = base.rgb * vc * diff + spec;
        if ((flags & F_ILLUM) != 0u) {
            col += imap.rgb * u_mat.illum.rgb;
        }
        // environment reflection (chrome, glass, polished floors)
        if ((flags & (F_REFLECT_MAP | F_REFLECT_SKY)) != 0u && u_frame.ambient_sky.w > 0.0) {
            let R = reflect(-V, n);
            var env: vec3<f32>;
            if ((flags & F_REFLECT_MAP) != 0u) {
                // D3D style sphere map in camera space (x right, y up, z forward; the map centre is the direction back at the viewer)
                let vx = normalize(u_frame.cam_right.xyz);
                let vy = normalize(u_frame.cam_up.xyz);
                let r = vec3<f32>(dot(R, vx), dot(R, vy), -dot(R, u_frame.cam_fwd.xyz));
                let m = 2.0 * sqrt(r.x * r.x + r.y * r.y + (r.z + 1.0) * (r.z + 1.0));
                let suv = vec2<f32>(r.x / max(m, 1e-4) + 0.5, -r.y / max(m, 1e-4) + 0.5);
                env = textureSampleLevel(t_env, samp, suv, 0.0).rgb;
            } else {
                env = sky_color(R);
            }
            var amount = u_mat.params.w * u_frame.ambient_sky.w;
            if ((flags & F_REFL_MASK) != 0u) { amount = amount * textureSample(t_rmask, samp, in.uv).r; }
            if ((flags & F_REFL_FALLOFF) != 0u) {
                let ndv = clamp(dot(geo_n, V), 0.0, 1.0);
                amount = amount * textureSampleLevel(t_falloff, samp, vec2<f32>(1.0 - ndv, 0.5), 0.0).r;
            }
            let add = env * u_mat.refl.rgb * amount;
            col += add;
            refl_lum = dot(add, vec3<f32>(0.3, 0.59, 0.11));
        }
        col = col * u_frame.sun_color.w;
    }
    if ((flags & F_FOG) != 0u && u_frame.fog_color.w > u_frame.fog_params.x) {
        let f = clamp((dist - u_frame.fog_params.x) / (u_frame.fog_color.w - u_frame.fog_params.x), 0.0, 1.0);
        col = mix(col, u_frame.fog_color.rgb, f);
    }
    // transparent reflective surfaces (glass) become more opaque where they reflect
    alpha = clamp(alpha + refl_lum * (1.0 - alpha), 0.0, 1.0);
    return vec4<f32>(col, alpha);
}

// ---- sky ---------------------------------------------------------------------------------------------
struct SkyOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) i: u32) -> SkyOut {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: SkyOut;
    o.ndc = p * 2.0 - 1.0;
    o.clip = vec4<f32>(o.ndc, 0.0, 1.0);
    return o;
}

@fragment
fn fs_sky(in: SkyOut) -> @location(0) vec4<f32> {
    let dir = normalize(u_frame.cam_fwd.xyz + in.ndc.x * u_frame.cam_right.xyz + in.ndc.y * u_frame.cam_up.xyz);
    return vec4<f32>(sky_color(dir), 1.0);
}
