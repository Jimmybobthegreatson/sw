//! Material binding: `.MAT` instance -> [`MaterialDesc`] (plain data) -> texture ids -> decoded RGBA
//! images (lazy, cached).
//!
//! The renderer is expected to map `MaterialDesc` onto its own shaders; the original engine
//! selects a DirectX9 `.fx` file (`Standard13.fx`, `Skin20.fx` ...) per material class, which is
//! *not* reproduced here (the class -> effect mapping is still unknown, see `docs/formats.md`).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use bm_formats::mat::{Mat, MatProp, MatValue};
use bm_formats::tex::{Tex, TexFormat};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlendMode {
    /// No blending (`BENA == 0`, or no `BMOD`).
    Opaque,
    /// `TRANS`: alpha blended.
    Trans,
    /// `TRANS_ON_OPAQUE`: alpha blended, drawn after the opaque pass (most foliage/glass).
    TransOnOpaque,
    /// `ADD` additive.
    Add,
    AddBeforeTrans,
    AddOnOpaque,
    TransAddOnOpaque,
    /// Shadow-volume / shadow-caster only materials (not visible geometry).
    Shadow,
    StaticShadow,
    Other(String),
}

impl BlendMode {
    fn from_str(s: &str) -> BlendMode {
        match s {
            "TRANS" => BlendMode::Trans,
            "TRANS_ON_OPAQUE" => BlendMode::TransOnOpaque,
            "ADD" => BlendMode::Add,
            "ADD_BEFORE_TRANS" => BlendMode::AddBeforeTrans,
            "ADD_ON_OPAQUE" => BlendMode::AddOnOpaque,
            "TRANSADD_ON_OPAQUE" => BlendMode::TransAddOnOpaque,
            "SHADOW" => BlendMode::Shadow,
            "STATICSHADOW" => BlendMode::StaticShadow,
            "" | "Opaque" => BlendMode::Opaque,
            o => BlendMode::Other(o.to_string()),
        }
    }

    pub fn is_additive(&self) -> bool {
        matches!(self, BlendMode::Add | BlendMode::AddBeforeTrans | BlendMode::AddOnOpaque | BlendMode::TransAddOnOpaque)
    }

    pub fn is_alpha_blended(&self) -> bool {
        matches!(self, BlendMode::Trans | BlendMode::TransOnOpaque | BlendMode::TransAddOnOpaque) || self.is_additive()
    }

    pub fn is_shadow(&self) -> bool {
        matches!(self, BlendMode::Shadow | BlendMode::StaticShadow)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CullMode {
    /// `DontCare` / unspecified: engine default (back-face culling).
    Default,
    OneSided,
    TwoSided,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileMode {
    Tiled,
    Mirrored,
    /// `NONE` / empty: clamp.
    Clamp,
}

impl TileMode {
    fn from_str(s: &str) -> TileMode {
        match s {
            "TILED" => TileMode::Tiled,
            "MIRRORED" => TileMode::Mirrored,
            _ => TileMode::Clamp,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderState {
    pub blend: BlendMode,
    /// `OPAC`: constant opacity multiplier.
    pub opacity: f32,
    /// `ATST`: alpha test enabled.
    pub alpha_test: bool,
    /// `AREF`: alpha test reference 0..255 (pass when `alpha * 255 > aref`... engine detail unverified).
    pub alpha_ref: u8,
    pub cull: CullMode,
    pub z_bias: i32,
    pub z_offset: f32,
    pub fog_enabled: bool,
}

impl Default for RenderState {
    fn default() -> Self {
        RenderState {
            blend: BlendMode::Opaque,
            opacity: 1.0,
            alpha_test: false,
            alpha_ref: 0,
            cull: CullMode::Default,
            z_bias: 0,
            z_offset: 0.0,
            fog_enabled: true,
        }
    }
}

/// A texture slot of a material (`mapDiffuse`, `mapNormal`, `mapSpecularMask`, ...).
#[derive(Clone, Debug, PartialEq)]
pub struct TextureBinding {
    pub slot: String,
    /// Id in the scene's `.TEX` file.
    pub texture_id: u32,
    pub enabled: bool,
    pub tile_u: TileMode,
    pub tile_v: TileMode,
}

/// Plain-data description of a material. Everything a renderer needs, nothing GPU specific.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialDesc {
    pub id: u32,
    pub name: String,
    /// `PATH` of effect materials (e.g. `ScatterBillboard.fx`), if any.
    pub shader: Option<String>,
    pub render_state: RenderState,
    /// Texture slots in file order (only slots that name a texture id).
    pub textures: Vec<TextureBinding>,
    /// Color parameters (`v4DiffuseColor`, `v4SpecularColor`, ...) of enabled `COLO` entries.
    pub colors: BTreeMap<String, [f32; 4]>,
    /// Float parameters (`v4SpecularPower`, `v4BumpScale`, `fRimLightPower`, ...) of enabled `FLTV` entries.
    pub floats: BTreeMap<String, Vec<f32>>,
    /// Feature switches (`BumpEnabled`, `SpecularEnabled`, `ReflectionEnabled` ...); only enabled `BOOL` entries.
    pub flags: BTreeMap<String, bool>,
}

impl MaterialDesc {
    /// Enabled texture id of a slot.
    pub fn texture(&self, slot: &str) -> Option<u32> {
        self.textures.iter().find(|t| t.enabled && t.slot == slot).map(|t| t.texture_id)
    }

    pub fn diffuse_texture(&self) -> Option<u32> {
        self.texture("mapDiffuse")
    }

    pub fn normal_texture(&self) -> Option<u32> {
        self.texture("mapNormal")
    }

    pub fn specular_mask_texture(&self) -> Option<u32> {
        self.texture("mapSpecularMask")
    }

    pub fn illumination_texture(&self) -> Option<u32> {
        self.texture("mapIllumination")
    }

    /// Base color factor (`v4DiffuseColor`, default white).
    pub fn diffuse_color(&self) -> [f32; 4] {
        self.colors.get("v4DiffuseColor").copied().unwrap_or([1.0; 4])
    }

    pub fn flag(&self, name: &str) -> bool {
        self.flags.get(name).copied().unwrap_or(false)
    }

    /// Shadow-only helper materials should not be drawn in the color pass.
    pub fn is_shadow_only(&self) -> bool {
        self.render_state.blend.is_shadow()
    }
}

fn int_of(p: &MatProp, tag: &str) -> Option<u32> {
    match p.find(tag) {
        Some(MatValue::Int(v)) => Some(*v),
        _ => None,
    }
}

fn str_of(p: &MatProp, tag: &str) -> Option<String> {
    match p.find(tag) {
        Some(MatValue::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

fn floats_of(p: &MatProp, tag: &str) -> Option<Vec<f32>> {
    match p.find(tag) {
        Some(MatValue::Floats(f)) => Some(f.clone()),
        Some(MatValue::Float(f)) => Some(vec![*f]),
        _ => None,
    }
}

/// Builds the description of material instance `id`. Header-only stub instances describe as blank (nothing to draw).
pub fn describe_material(mat: &Mat, id: u32) -> Option<MaterialDesc> {
    let inst = mat.instance(id)?;
    let props = inst.props(mat);
    let mut d = MaterialDesc {
        id,
        name: inst.name.clone(),
        shader: inst.shader(mat),
        render_state: RenderState::default(),
        textures: Vec::new(),
        colors: BTreeMap::new(),
        floats: BTreeMap::new(),
        flags: BTreeMap::new(),
    };
    for p in &props {
        let name = str_of(p, "NAME").unwrap_or_default();
        let enabled = int_of(p, "ENAB").unwrap_or(1) != 0;
        match p.tag.as_str() {
            "RSTA" => {
                let rs = &mut d.render_state;
                let bena = int_of(p, "BENA").unwrap_or(0) != 0;
                if bena {
                    if let Some(b) = str_of(p, "BMOD") {
                        rs.blend = BlendMode::from_str(&b);
                    }
                } else if let Some(b) = str_of(p, "BMOD") {
                    // shadow materials keep their BMOD without BENA
                    let m = BlendMode::from_str(&b);
                    if m.is_shadow() {
                        rs.blend = m;
                    }
                }
                if let Some(MatValue::Float(o)) = p.find("OPAC") {
                    rs.opacity = *o;
                }
                rs.alpha_test = int_of(p, "ATST").unwrap_or(0) != 0;
                rs.alpha_ref = int_of(p, "AREF").unwrap_or(0).min(255) as u8;
                rs.cull = match str_of(p, "CULL").as_deref() {
                    Some("OneSided") => CullMode::OneSided,
                    Some("TwoSided") => CullMode::TwoSided,
                    _ => CullMode::Default,
                };
                rs.z_bias = int_of(p, "ZBIA").unwrap_or(0) as i32;
                if let Some(MatValue::Float(z)) = p.find("ZOFF") {
                    rs.z_offset = *z;
                }
                rs.fog_enabled = int_of(p, "FENA").unwrap_or(1) != 0;
            }
            "TEXT" => {
                if let Some(tid) = int_of(p, "TXID") {
                    d.textures.push(TextureBinding {
                        slot: name,
                        texture_id: tid,
                        enabled,
                        tile_u: TileMode::from_str(&str_of(p, "TILU").unwrap_or_default()),
                        tile_v: TileMode::from_str(&str_of(p, "TILV").unwrap_or_default()),
                    });
                }
            }
            "COLO" if enabled => {
                if let Some(f) = floats_of(p, "VALU") {
                    if f.len() >= 4 {
                        d.colors.insert(name, [f[0], f[1], f[2], f[3]]);
                    }
                }
            }
            "FLTV" if enabled => {
                if let Some(f) = floats_of(p, "VALU") {
                    d.floats.insert(name, f);
                }
            }
            "BOOL" if enabled => {
                if let Some(v) = int_of(p, "VALU") {
                    d.flags.insert(name, v != 0);
                }
            }
            _ => {}
        }
    }
    Some(d)
}

/// A decoded texture level 0.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub id: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8, top row first.
    pub rgba: Vec<u8>,
    /// Source format (`DXT1`, `RGBA`, ...).
    pub source_format: String,
    /// Mip count in the source file.
    pub mip_count: u32,
}

impl Image {
    pub fn has_alpha(&self) -> bool {
        self.rgba.chunks_exact(4).any(|p| p[3] != 255)
    }
}

/// Lazy material + texture resolver for one scene (`.MAT` + `.TEX`).
pub struct MaterialLibrary {
    mat: Arc<Mat>,
    tex: Arc<Tex>,
    descs: Mutex<HashMap<u32, Option<Arc<MaterialDesc>>>>,
    images: Mutex<HashMap<u32, Option<Arc<Image>>>>,
}

impl MaterialLibrary {
    pub fn new(mat: Mat, tex: Tex) -> MaterialLibrary {
        MaterialLibrary { mat: Arc::new(mat), tex: Arc::new(tex), descs: Default::default(), images: Default::default() }
    }

    pub fn mat(&self) -> &Mat {
        &self.mat
    }

    pub fn tex(&self) -> &Tex {
        &self.tex
    }

    /// All material instance ids.
    pub fn ids(&self) -> Vec<u32> {
        self.mat.instances.iter().map(|i| i.id).collect()
    }

    /// Material description (cached). `None` when the id has no instance.
    pub fn desc(&self, id: u32) -> Option<Arc<MaterialDesc>> {
        let mut g = self.descs.lock().unwrap();
        g.entry(id).or_insert_with(|| describe_material(&self.mat, id).map(Arc::new)).clone()
    }

    /// Which real material the header-only stub instances among `ids` stand for in the weapon model group `model`
    /// (`Rifle_SG552_01`, `Custom_Pistol`; the `Ground01` link's group name). Returns `stub id -> instance id`.
    ///
    /// The stubs carry no data. Their neighbours in the table show the pattern (`docs/models.md`, "stub materials"): a stub is
    /// a further use of the block in front of it when that is the model's own material (`SniperRifle_SakoTRG_01` body) or a
    /// generic part sheet (`WeaponParts/Metal_02`, `Leather_01`); when the block in front belongs to another weapon (the SG552
    /// body follows the dart ammo) or is a decal sheet (`Sign_*`, alpha tested labels: magazines), the material named after the
    /// model (`Weapons/<model>_WPN_H3`, without `_Silenced`) is used, else the black `Leather_01` for decal sheets, else the
    /// block in front.
    pub fn stub_remap(&self, model: &str, ids: impl IntoIterator<Item = u32>) -> HashMap<u32, u32> {
        let m = self.mat();
        let real = |id: u32| m.instance(id).filter(|i| !i.is_stub());
        let by_name = |wanted: &str| m.instances.iter().find(|i| !i.is_stub() && i.name.to_ascii_lowercase() == wanted).map(|i| i.id);
        let model_l = model.to_ascii_lowercase();
        let own = [model_l.clone(), model_l.replace("_silenced", "")].iter().find_map(|n| by_name(&format!("weapons/{n}_wpn_h3")));
        let leather = by_name("weapons/weaponparts/leather_01_wpn_h3");
        let mut out = HashMap::new();
        for id in ids {
            if !m.instance(id).is_some_and(|i| i.is_stub()) {
                continue;
            }
            let Some(prev) = (1..id).rev().find(|&p| real(p).is_some()) else { continue };
            let pname = real(prev).map(|i| i.name.to_ascii_lowercase()).unwrap_or_default();
            let decal = pname.contains("/sign_");
            let generic = pname.starts_with("weapons/weaponparts/") && !decal;
            let prev_is_own = pname.contains(model_l.trim_start_matches("custom_")) && !model_l.is_empty();
            let target = if prev_is_own || generic {
                prev
            } else if let Some(o) = own {
                o
            } else if decal {
                leather.unwrap_or(prev)
            } else {
                prev
            };
            out.insert(id, target);
        }
        out
    }

    /// Decoded texture (mip 0, RGBA8; cached). `None` for unknown ids / id lists / undecodable formats.
    pub fn image(&self, texture_id: u32) -> Option<Arc<Image>> {
        let mut g = self.images.lock().unwrap();
        g.entry(texture_id)
            .or_insert_with(|| {
                let t = self.tex.image(texture_id)?;
                let rgba = t.decode_rgba(0).ok()?;
                let (w, h) = (t.width as u32, t.height as u32);
                if rgba.len() != (w * h * 4) as usize {
                    return None;
                }
                Some(Arc::new(Image {
                    id: texture_id,
                    name: t.name.clone(),
                    width: w,
                    height: h,
                    rgba,
                    source_format: format_name(t.format),
                    mip_count: t.mips.len() as u32,
                }))
            })
            .clone()
    }

    /// Convenience: the decoded diffuse texture of a material.
    pub fn diffuse_image(&self, material_id: u32) -> Option<Arc<Image>> {
        self.image(self.desc(material_id)?.diffuse_texture()?)
    }
}

fn format_name(f: TexFormat) -> String {
    f.name()
}
