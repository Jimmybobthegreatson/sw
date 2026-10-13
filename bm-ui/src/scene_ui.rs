//! UI resources stored *inside* a scene (`Scenes/<x>.ZIP`): fonts (`ZFONT` nodes), sprites (`ZWINPIC` nodes), the
//! scene's `.LOC` strings and `Bitmaps/..` textures. Loaded on demand and registered in a [`TextureStore`].

use std::collections::HashMap;

use bm_formats::loader::SceneFiles;
use bm_formats::prm::Prm;
use bm_formats::prp::Value;
use bm_formats::scene::SceneGraph;
use bm_formats::tex::Tex;
use bm_vfs::Vfs;

use crate::draw::Blend;
use crate::font::{BitmapFont, FontPage, FontSet};
use crate::sprite::{parse_winpic_chunk, Anchor, Sprite};
use crate::strings::Strings;
use crate::texture::{Image, TextureId, TextureStore};

pub const TYPE_ZWINPIC: u32 = 0x200046;
pub const TYPE_ZFONT: u32 = 0x20004c;
/// `ZTEXT` line object (menu text lines; subtitles).
pub const TYPE_ZTEXT: u32 = 0x200038;

#[derive(Debug)]
pub enum SceneUiError {
    Load(String),
}

impl std::fmt::Display for SceneUiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SceneUiError::Load(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for SceneUiError {}

pub struct SceneUi {
    pub base: String,
    graph: SceneGraph,
    prm: Prm,
    tex: Tex,
    pub loc: Option<bm_formats::loc::Loc>,
    tex_cache: HashMap<u32, TextureId>,
    sprite_cache: HashMap<String, Option<Sprite>>,
    /// lower-case node name -> node indices (ZWINPIC nodes and the groups that contain them).
    name_index: HashMap<String, Vec<usize>>,
}

impl SceneUi {
    pub fn load(vfs: &Vfs, scene: &str) -> Result<SceneUi, SceneUiError> {
        let sf = SceneFiles::load(vfs, scene).map_err(|e| SceneUiError::Load(e.to_string()))?;
        let graph = sf.graph().map_err(|e| SceneUiError::Load(e.to_string()))?;
        Ok(SceneUi::from_files(sf, graph))
    }

    pub fn from_files(sf: SceneFiles, graph: SceneGraph) -> SceneUi {
        let mut name_index: HashMap<String, Vec<usize>> = HashMap::new();
        for n in &graph.nodes {
            let interesting = n.type_id == TYPE_ZWINPIC
                || n.children.iter().any(|&c| graph.nodes[c].type_id == TYPE_ZWINPIC);
            if interesting {
                name_index.entry(n.name.to_ascii_lowercase()).or_default().push(n.index);
            }
        }
        SceneUi {
            base: sf.base.clone(),
            graph,
            prm: sf.prm,
            tex: sf.tex,
            loc: sf.loc,
            tex_cache: HashMap::new(),
            sprite_cache: HashMap::new(),
            name_index,
        }
    }

    pub fn graph(&self) -> &SceneGraph {
        &self.graph
    }

    /// Adds this scene's `.LOC` tree to `strings`.
    pub fn add_strings(&self, strings: &mut Strings) {
        if let Some(l) = &self.loc {
            strings.add_loc(l.clone());
        }
    }

    /// Registers (once) scene texture `tex_id` in the store.
    pub fn texture(&mut self, store: &mut TextureStore, tex_id: u32) -> Option<TextureId> {
        if let Some(&t) = self.tex_cache.get(&tex_id) {
            return Some(t);
        }
        let img = self.tex.image(tex_id)?;
        let rgba = img.decode_rgba(0).ok()?;
        let (w, h) = img.mip_size(0);
        let mut image = Image::new(w, h, rgba, format!("{}#{}:{}", self.base, tex_id, img.name));
        image.nearest = false;
        let id = store.add(image);
        self.tex_cache.insert(tex_id, id);
        Some(id)
    }

    /// Texture by its TEX image name (`Bitmaps/Screens/MainMenu/HMBMLogo_01`); first match.
    pub fn texture_by_name(&mut self, store: &mut TextureStore, name: &str) -> Option<TextureId> {
        let id = self.tex.images().find(|i| i.name.eq_ignore_ascii_case(name))?.id;
        self.texture(store, id)
    }

    pub fn texture_names(&self) -> Vec<(u32, String)> {
        self.tex.images().map(|i| (i.id, i.name.clone())).collect()
    }

    /// Image chunk `[0x20101, tex_id, ..]` -> scene texture.
    fn image_chunk_tex(&self, chunk: u32) -> Option<u32> {
        let w = self.prm.chunk_words(chunk as usize)?;
        (w.len() == 8 && w[0] == 0x20101).then_some(w[1])
    }

    /// Loads all fonts of the scene (`ZFONT` nodes `Text`, `Menu`, `Header`), registers the page textures and
    /// returns them in a set. Fonts whose chunk fails to decode are skipped with a warning.
    pub fn load_fonts(&mut self, store: &mut TextureStore) -> FontSet {
        let mut set = FontSet::new();
        let nodes: Vec<(String, usize)> = self
            .graph
            .nodes
            .iter()
            .filter(|n| n.type_id == TYPE_ZFONT)
            .filter_map(|n| match n.props.iter().rev().nth(1) {
                Some(Value::I32(c)) if *c >= 0 => Some((n.name.clone(), *c as usize)),
                _ => None,
            })
            .collect();
        // Page textures: the glyph records reference page *descriptor chunks* whose link to a TEX image is not
        // decoded (docs/ui.md); the baked pages are the `Fonts/..#n` images in order, split across the fonts by
        // their page counts, and a font's distinct page chunks (ascending) map to its pages in order.
        let mut font_imgs: Vec<&bm_formats::tex::TexImage> = self.tex.images().filter(|i| i.name.starts_with("Fonts/")).collect();
        font_imgs.sort_by_key(|i| i.name.rsplit('#').next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(u32::MAX));
        let mut page_offset = 0usize;
        for (name, chunk) in nodes {
            let Some(words) = self.prm.chunk_words(chunk) else { continue };
            let npages = words.get(7).copied().unwrap_or(0) as usize;
            let nglyph = words.get(4).copied().unwrap_or(0) as usize;
            let mut refs: Vec<u32> = (0..nglyph).filter_map(|i| words.get(8 + i * 5 + 1).copied()).collect();
            refs.sort_unstable();
            refs.dedup();
            let base = page_offset;
            page_offset += npages;
            let imgs = &font_imgs;
            let base_name = self.base.clone();
            let font = BitmapFont::from_chunk_words(&name, &words, |page_chunk| {
                let rank = refs.iter().position(|&r| r == page_chunk)?;
                let img = imgs.get(base + rank)?;
                let (pw, ph) = img.mip_size(0);
                Some(FontPage { width: pw, height: ph, rgba: img.decode_rgba(0).ok()?, label: format!("{base_name}:font:{name}:{}", img.name), texture: None })
            });
            match font {
                Some(mut f) => {
                    f.register(store);
                    set.insert(f);
                }
                None => log::warn!("font node {name} (chunk {chunk}) did not decode"),
            }
        }
        // the main UI font is `Menu` (menu XML default); fall back to whatever loaded first
        if set.get("Menu").is_some() {
            set.set_default("Menu");
        }
        set
    }

    /// Finds a sprite by node name: a `ZWINPIC` node, or a group whose (first) `ZWINPIC` child is used - the
    /// engine's `Graphic="HitmanBackground"` references name such groups. `A/B` selects child `B` of group `A`.
    pub fn sprite(&mut self, store: &mut TextureStore, name: &str) -> Option<&Sprite> {
        let key = name.to_ascii_lowercase();
        if !self.sprite_cache.contains_key(&key) {
            let built = self.build_sprite(store, name);
            self.sprite_cache.insert(key.clone(), built);
        }
        self.sprite_cache.get(&key).and_then(|s| s.as_ref())
    }

    fn build_sprite(&mut self, store: &mut TextureStore, name: &str) -> Option<Sprite> {
        let (group, child) = match name.split_once('/') {
            Some((g, c)) => (g, Some(c)),
            None => (name, None),
        };
        let cands = self.name_index.get(&group.to_ascii_lowercase())?.clone();
        let mut node_idx = None;
        for &i in &cands {
            let n = &self.graph.nodes[i];
            if n.type_id == TYPE_ZWINPIC && child.is_none() {
                node_idx = Some(i);
                break;
            }
            let kid = n.children.iter().copied().find(|&c| {
                let k = &self.graph.nodes[c];
                k.type_id == TYPE_ZWINPIC && child.map(|cn| k.name.eq_ignore_ascii_case(cn)).unwrap_or(true)
            });
            if kid.is_some() {
                node_idx = kid;
                break;
            }
        }
        self.sprite_from_node(store, node_idx?)
    }

    pub fn sprite_from_node(&mut self, store: &mut TextureStore, idx: usize) -> Option<Sprite> {
        let node = &self.graph.nodes[idx];
        if node.type_id != TYPE_ZWINPIC {
            return None;
        }
        let p = &node.props;
        let n = p.len();
        if n < 6 {
            return None;
        }
        let (Value::I32(w), Value::I32(h), Value::I32(chunk)) = (&p[n - 3], &p[n - 2], &p[n - 1]) else { return None };
        let tok = |v: &Value| if let Value::Token(t) = v { t.clone() } else { String::new() };
        let anchor = Anchor::from_token(&tok(&p[n - 6]));
        let blend = if tok(&p[n - 5]).contains("ADD") { Blend::Additive } else { Blend::Alpha };
        let layer = match &p[n - 7] {
            Value::I8(l) => *l as i32,
            _ => 0,
        };
        let (w, h, chunk) = (*w as f32, *h as f32, *chunk as usize);
        let name = node.name.clone();
        let words = self.prm.chunk_words(chunk)?;
        let mut tex_ids = Vec::new();
        let this: &SceneUi = self;
        let mut parts = parse_winpic_chunk(&words, |c| {
            let t = this.image_chunk_tex(c)?;
            tex_ids.push(t);
            Some(TextureId(t)) // placeholder, patched below
        })?;
        for (part, t) in parts.iter_mut().zip(&tex_ids) {
            part.texture = self.texture(store, *t).unwrap_or(TextureId::WHITE);
        }
        Some(Sprite { name, size: [w, h], parts, blend, anchor, layer })
    }

    /// Names of all sprite groups / pictures (for tooling).
    pub fn sprite_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.name_index.keys().cloned().collect();
        v.sort();
        v
    }
}
