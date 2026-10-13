//! Glacier -> Bevy conversions: coordinate system, meshes, textures and materials.
//!
//! Glacier is left-handed, Y up, centimetres, clockwise front faces. Bevy is right-handed, Y up, metres,
//! counter-clockwise front faces. Mirroring Z (`z -> -z`) and scaling by 0.01 maps one onto the other; a mirror keeps
//! the on-screen winding; the native triangle order turns out to already be counter-clockwise after the mirror, so
//! the indices are used as stored (reversing them culled every floor and showed wall interiors).

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bm_models::material::{BlendMode, CullMode, MaterialDesc, TileMode};
use bm_models::mesh::Mesh as BmMesh;
use bm_models::{Mat4 as BmMat4, Scene};
use std::collections::HashMap;

/// Centimetres -> metres.
pub const S: f32 = 0.01;

/// Largest texture edge kept (higher mip levels are dropped, like the old renderer's `max_texture_size`).
const MAX_TEXTURE: u32 = 1024;

/// Vertex colours are authored as display (sRGB) bytes; Bevy lights in linear space.
pub fn srgb_to_linear(b: u8, gain: f32) -> f32 {
    let c = b as f32 / 255.0 * gain;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

pub fn pos(v: [f32; 3]) -> [f32; 3] {
    [v[0] * S, v[1] * S, -v[2] * S]
}

pub fn dir(v: [f32; 3]) -> [f32; 3] {
    [v[0], v[1], -v[2]]
}

pub fn vec3(v: [f32; 3]) -> Vec3 {
    Vec3::from_array(pos(v))
}

pub fn dir3(v: [f32; 3]) -> Vec3 {
    Vec3::from_array(dir(v))
}

/// Column-major native matrix -> Bevy matrix (conjugated by the Z mirror, translation in metres).
pub fn mat4(m: &BmMat4) -> Mat4 {
    let f = [1.0f32, 1.0, -1.0, 1.0];
    let mut c = [0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            let mut v = m.0[col][row] * f[row] * f[col];
            if col == 3 && row < 3 {
                v *= S;
            }
            c[col * 4 + row] = v;
        }
    }
    Mat4::from_cols_array(&c)
}

/// Native matrix -> Bevy transform; degenerate matrices collapse to a zero-scale transform instead of NaN.
pub fn transform(m: &BmMat4) -> Transform {
    let m = mat4(m);
    if !m.is_finite() || m.determinant().abs() < 1e-12 {
        return Transform::from_translation(m.w_axis.truncate()).with_scale(Vec3::ZERO);
    }
    Transform::from_matrix(m)
}

/// Meshes that are drawn by the old renderer. Everything else (collision, shadow-only, helper) is left out.
pub fn is_helper_material(name: &str) -> bool {
    name.starts_with("_Glacier/") || name.starts_with("AlwaysInShadow") || name.starts_with("Screens/")
}

pub fn is_blank_material(d: &MaterialDesc) -> bool {
    (d.name.is_empty() && d.diffuse_texture().is_none()) || d.name == "Bad"
}

/// Native mesh data -> Bevy mesh (static pose). Skinned meshes keep their bind pose until [`skin_into`] is applied.
/// `color_gain` scales the authored (display space) vertex colours: the untextured sky dome is drawn with the game's `sky_gain`.
pub fn build_mesh(m: &BmMesh, dynamic: bool, color_gain: f32) -> Mesh {
    let usage = if dynamic { RenderAssetUsages::default() } else { RenderAssetUsages::RENDER_WORLD };
    let v = &m.vertices;
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, usage);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, v.positions.iter().map(|&p| pos(p)).collect::<Vec<_>>());
    let has_normals = v.normals.len() == v.positions.len();
    if has_normals {
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, v.normals.iter().map(|&n| dir(n)).collect::<Vec<_>>());
    }
    let uvs: Vec<[f32; 2]> = if v.uvs.len() == v.positions.len() { v.uvs.clone() } else { vec![[0.0; 2]; v.positions.len()] };
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    if !m.is_skinned() && v.tangents.len() == v.positions.len() {
        // mirroring Z flips the handedness of the tangent frame
        mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, v.tangents.iter().map(|t| [t[0], t[1], -t[2], -t[3]]).collect::<Vec<_>>());
    }
    if v.colors.len() == v.positions.len() {
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_COLOR,
            v.colors.iter().map(|c| [srgb_to_linear(c[0], color_gain), srgb_to_linear(c[1], color_gain), srgb_to_linear(c[2], color_gain), c[3] as f32 / 255.0]).collect::<Vec<_>>(),
        );
    }
    let mut idx = Vec::with_capacity(m.indices.len());
    for t in m.indices.chunks_exact(3) {
        idx.extend_from_slice(&[t[0], t[1], t[2]]);
    }
    mesh.insert_indices(Indices::U32(idx));
    if !has_normals {
        mesh.compute_smooth_normals();
    }
    mesh
}

/// CPU skinning of one mesh: `bones` are the skeleton's skinning matrices in native space (as the old shader used them).
pub fn skin_into(mesh: &mut Mesh, src: &BmMesh, bones: &[BmMat4]) {
    let v = &src.vertices;
    let n = v.positions.len();
    let mut pos_out = Vec::with_capacity(n);
    let mut nrm_out = Vec::with_capacity(n);
    for i in 0..n {
        let p = v.positions[i];
        let nm = v.normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
        let (j, w) = (v.joints[i], v.weights[i]);
        let mut sp = [0f32; 3];
        let mut sn = [0f32; 3];
        for k in 0..4 {
            if w[k] == 0.0 {
                continue;
            }
            let Some(b) = bones.get(j[k] as usize) else { continue };
            let tp = b.transform_point(p);
            let tn = b.transform_vector(nm);
            for a in 0..3 {
                sp[a] += tp[a] * w[k];
                sn[a] += tn[a] * w[k];
            }
        }
        pos_out.push(pos(sp));
        let l = (sn[0] * sn[0] + sn[1] * sn[1] + sn[2] * sn[2]).sqrt().max(1e-6);
        nrm_out.push(dir([sn[0] / l, sn[1] / l, sn[2] / l]));
    }
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos_out);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nrm_out);
}

/// Decoded game textures as Bevy images (one per texture id and tiling mode) plus the materials built from them.
pub struct Assets3d {
    images: HashMap<(u32, u8, u8), Option<Handle<Image>>>,
    materials: HashMap<(u32, bool), Option<Handle<StandardMaterial>>>,
    maps: HashMap<(u32, u8, u8), Option<Handle<Image>>>,
}

impl Assets3d {
    pub fn new() -> Self {
        Assets3d { images: HashMap::new(), materials: HashMap::new(), maps: HashMap::new() }
    }

    fn image(&mut self, scene: &Scene, images: &mut Assets<Image>, id: u32, u: TileMode, v: TileMode) -> Option<Handle<Image>> {
        let key = (id, u as u8, v as u8);
        self.images
            .entry(key)
            .or_insert_with(|| {
                let img = scene.materials.image(id)?;
                Some(images.add(to_bevy_image(&img.rgba, img.width, img.height, u, v, true)))
            })
            .clone()
    }

    /// The Bevy material of a `.MAT` instance; `None` when the instance is not drawn (shadow / helper / blank).
    pub fn material(&mut self, scene: &Scene, images: &mut Assets<Image>, mats: &mut Assets<StandardMaterial>, id: u32, skinned: bool) -> Option<Handle<StandardMaterial>> {
        if let Some(m) = self.materials.get(&(id, skinned)) {
            return m.clone();
        }
        let desc = scene.materials.desc(id);
        let out = desc.and_then(|d| {
            if d.is_shadow_only() || is_helper_material(&d.name) || is_blank_material(&d) {
                return None;
            }
            Some(mats.add(self.standard(scene, images, &d, skinned)))
        });
        self.materials.insert((id, skinned), out.clone());
        out
    }

    /// Normal map: the game stores x/y only (`U8V8`, or the first two channels of other formats); z is rebuilt here.
    fn normal_map(&mut self, scene: &Scene, images: &mut Assets<Image>, id: u32, u: TileMode, v: TileMode) -> Option<Handle<Image>> {
        let key = (id, u as u8, v as u8 | 0x80);
        self.maps
            .entry(key)
            .or_insert_with(|| {
                let img = scene.materials.image(id)?;
                let mut rgba = img.rgba.clone();
                for p in rgba.chunks_exact_mut(4) {
                    let (x, y) = (p[0] as f32 / 255.0 * 2.0 - 1.0, p[1] as f32 / 255.0 * 2.0 - 1.0);
                    let z = (1.0 - x * x - y * y).max(0.0).sqrt();
                    p[2] = ((z * 0.5 + 0.5) * 255.0).round() as u8;
                    p[3] = 255;
                }
                Some(images.add(to_bevy_image(&rgba, img.width, img.height, u, v, false)))
            })
            .clone()
    }

    /// Roughness map (green channel) from a specular mask: `1 - mask * (1 - base)`, so masked-out texels are rough.
    fn roughness_map(&mut self, scene: &Scene, images: &mut Assets<Image>, id: u32, u: TileMode, v: TileMode, base: f32) -> Option<Handle<Image>> {
        let q = (base * 20.0).round() as u8;
        let key = (id, u as u8 | 0x40, v as u8 | (q << 2));
        self.maps
            .entry(key)
            .or_insert_with(|| {
                let img = scene.materials.image(id)?;
                let r0 = q as f32 / 20.0;
                let mut rgba = Vec::with_capacity(img.rgba.len());
                for p in img.rgba.chunks_exact(4) {
                    let mask = p[0] as f32 / 255.0;
                    let g = ((1.0 - mask * (1.0 - r0)) * 255.0).round() as u8;
                    rgba.extend_from_slice(&[255, g, 0, 255]);
                }
                Some(images.add(to_bevy_image(&rgba, img.width, img.height, u, v, false)))
            })
            .clone()
    }

    fn standard(&mut self, scene: &Scene, images: &mut Assets<Image>, d: &MaterialDesc, skinned: bool) -> StandardMaterial {
        let rs = &d.render_state;
        if let Ok(f) = std::env::var("BM_MATDBG") {
            if d.name.to_ascii_lowercase().contains(&f.to_ascii_lowercase()) {
                if let Some(i) = d.diffuse_texture().and_then(|t| scene.materials.image(t)) {
                    let n = (i.rgba.len() / 4) as f32;
                    log::info!("MAT tex {}x{} {} avg {:?}", i.width, i.height, i.source_format, [0, 1, 2, 3].map(|c| i.rgba.iter().skip(c).step_by(4).map(|&v| v as f32).sum::<f32>() / n));
                }
                log::info!("MAT {} {:?} shader {:?} rs {:?} colors {:?} floats {:?} flags {:?} tex {:?}", d.id, d.name, d.shader, d.render_state, d.colors, d.floats, d.flags, d.textures.iter().map(|t| (t.slot.clone(), t.texture_id, t.enabled)).collect::<Vec<_>>());
            }
        }
        let tile = |slot: &str| d.textures.iter().find(|t| t.enabled && t.slot == slot).map(|t| (t.tile_u, t.tile_v)).unwrap_or((TileMode::Tiled, TileMode::Tiled));
        let diffuse_id = d.diffuse_texture();
        // a diffuse map stored in the two-channel U8V8 normal-map format carries no colour: draw a neutral dark grey
        let flat = diffuse_id.is_some_and(|t| scene.materials.image(t).is_some_and(|i| i.source_format == "U8V8"));
        let (tu, tv) = tile("mapDiffuse");
        let base_tex = diffuse_id.filter(|_| !flat).and_then(|t| self.image(scene, images, t, tu, tv));
        let emissive_tex = d.illumination_texture().and_then(|t| {
            let (tu, tv) = tile("mapIllumination");
            self.image(scene, images, t, tu, tv)
        });
        let c = d.diffuse_color();
        let opacity = if rs.blend == BlendMode::Opaque { 1.0 } else { rs.opacity };
        let base = if flat { [0.12, 0.12, 0.12, 1.0] } else { c };
        let alpha_mode = if rs.alpha_test {
            AlphaMode::Mask((rs.alpha_ref as f32 / 255.0).max(0.004))
        } else {
            match &rs.blend {
                BlendMode::Opaque => AlphaMode::Opaque,
                b if b.is_additive() => AlphaMode::Add,
                BlendMode::Trans | BlendMode::TransOnOpaque => AlphaMode::Blend,
                _ => AlphaMode::Opaque,
            }
        };
        // tangent-space normal map (skinned meshes are posed on the CPU and carry no tangent frame)
        let normal_tex = d.normal_texture().filter(|_| !skinned && d.flags.get("BumpEnabled") != Some(&false)).and_then(|t| {
            let (nu, nv) = tile("mapNormal");
            self.normal_map(scene, images, t, nu, nv)
        });
        // Blinn-Phong power -> GGX roughness; the specular mask modulates it per pixel
        let spec_on = d.flag("SpecularEnabled");
        let power = d.floats.get("v4SpecularPower").and_then(|v| v.first().copied()).unwrap_or(20.0).max(1.0);
        let level = d.floats.get("vSpecularLevel").and_then(|v| v.first().copied()).unwrap_or(0.5);
        let spec_col = d.colors.get("v4SpecularColor").map(|c| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]).unwrap_or(1.0);
        let rough = (2.0 / (power + 2.0)).sqrt().clamp(0.12, 1.0);
        let spec_mask = d.specular_mask_texture().filter(|&t| spec_on && !scene.materials.image(t).is_some_and(|i| i.source_format == "U8V8")).and_then(|t| {
            let (su, sv) = tile("mapSpecularMask");
            self.roughness_map(scene, images, t, su, sv, rough)
        });
        // Cloud layers are data-wise additive at 25 %, but the original draws them with the sky gain over the dome and they
        // read as dark, patchy clouds: blend them instead of adding.
        let _cloud = d.name.contains("Backdrop/Clouds");
        let alpha_mode = alpha_mode;
        let two_sided = rs.cull == CullMode::TwoSided;
        let has_mask = spec_mask.is_some();
        StandardMaterial {
            base_color: Color::linear_rgba(base[0], base[1], base[2], (base[3] * opacity).clamp(0.0, 1.0)),
            base_color_texture: base_tex,
            // Bevy scales emissive by the view exposure (1 / 997 at the default EV100), so a display-space 1.0 needs that factor back
            emissive: if emissive_tex.is_some() && d.flag("IlluminationEnabled") {
                // the illumination texture is tinted by the material's `v4IlluminationColor` (display space)
                let c = d.colors.get("v4IlluminationColor").copied().unwrap_or([1.0; 4]);
                let lin = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
                LinearRgba::rgb(lin(c[0]) * 997.0, lin(c[1]) * 997.0, lin(c[2]) * 997.0)
            } else {
                LinearRgba::BLACK
            },
            emissive_texture: if d.flag("IlluminationEnabled") { emissive_tex } else { None },
            alpha_mode,
            double_sided: two_sided,
            cull_mode: if two_sided { None } else { Some(bevy::render::render_resource::Face::Back) },
            normal_map_texture: normal_tex,
            metallic_roughness_texture: spec_mask,
            // with a mask the texture's green channel already carries the roughness
            perceptual_roughness: if spec_on && !has_mask && !rs.alpha_test { rough } else { 1.0 },
            // alpha-tested foliage cards must not mirror the sun (they turned white at grazing angles)
            reflectance: if spec_on && !rs.alpha_test { (level * spec_col * 0.25).clamp(0.0, 0.35) } else { 0.0 },
            metallic: 0.0,
            fog_enabled: rs.fog_enabled,
            ..default()
        }
    }
}

fn address(t: TileMode) -> ImageAddressMode {
    match t {
        TileMode::Tiled => ImageAddressMode::Repeat,
        TileMode::Mirrored => ImageAddressMode::MirrorRepeat,
        TileMode::Clamp => ImageAddressMode::Repeat,
    }
}

/// RGBA8 -> Bevy image with a CPU-built box-filtered mip chain (Bevy does not generate mips), capped at [`MAX_TEXTURE`].
fn to_bevy_image(rgba: &[u8], w: u32, h: u32, u: TileMode, v: TileMode, srgb: bool) -> Image {
    let mut levels: Vec<(u32, u32, Vec<u8>)> = vec![(w, h, rgba.to_vec())];
    while let Some((lw, lh, data)) = levels.last() {
        if *lw == 1 && *lh == 1 {
            break;
        }
        let (nw, nh) = ((lw / 2).max(1), (lh / 2).max(1));
        let mut out = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                let (x0, y0) = ((x * 2).min(lw - 1), (y * 2).min(lh - 1));
                let (x1, y1) = ((x * 2 + 1).min(lw - 1), (y * 2 + 1).min(lh - 1));
                for c in 0..4 {
                    let s = |xx: u32, yy: u32| data[((yy * lw + xx) * 4) as usize + c] as u32;
                    out[((y * nw + x) * 4) as usize + c] = ((s(x0, y0) + s(x1, y0) + s(x0, y1) + s(x1, y1) + 2) / 4) as u8;
                }
            }
        }
        levels.push((nw, nh, out));
    }
    let skip = levels.iter().take_while(|(lw, lh, _)| *lw > MAX_TEXTURE || *lh > MAX_TEXTURE).count().min(levels.len() - 1);
    let levels = &levels[skip..];
    let (bw, bh, _) = levels[0];
    let mut data = Vec::new();
    for (_, _, d) in levels {
        data.extend_from_slice(d);
    }
    let mut img = Image::new(Extent3d { width: bw, height: bh, depth_or_array_layers: 1 }, TextureDimension::D2, data, if srgb { TextureFormat::Rgba8UnormSrgb } else { TextureFormat::Rgba8Unorm }, RenderAssetUsages::RENDER_WORLD);
    img.texture_descriptor.mip_level_count = levels.len() as u32;
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: address(u),
        address_mode_v: address(v),
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..default()
    });
    img
}
