//! [`UiSystem`]: the high-level facade used by the application. Owns the [`UiContext`], loads menus / HUD /
//! screens / fonts / string tables from the VFS (so loose mod files override the game's) and hot-reloads them.
//!
//! Mod override points (all paths are VFS paths, i.e. relative to a mod directory):
//!
//! | path | effect |
//! |---|---|
//! | `menu/<name>.xml` | replaces a shipped menu document (`frontend`, `ingame`, `briefing`, ...) |
//! | `menu/<name>.add.xml` | merged on top of the shipped / replaced document (new or replaced windows, styles) |
//! | `Fonts/<Name>.fnt` + PNG pages | replaces / adds a bitmap font (BMFont text format) |
//! | `Localisation/<lang>.json|ron`, `Localisation/<lang>/*` | string tables (flat `key -> text`) |
//! | `UI/hud.ron` / `UI/hud.json` | replaces the HUD layout |
//! | `UI/Screens/*.ron|json` | new screens |
//! | `UI/Images/*.png` | textures referenced by screens |
//! | `Config/input.ron` | default key bindings (the user's own file in the config dir wins) |
//! | `Shaders/WGSL/ui.wgsl` | UI shader ([`crate::gpu::UiRenderer::poll_hot_reload`]) |

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use bm_vfs::{Source, Vfs};

use crate::context::UiContext;
use crate::custom::ScreenLibrary;
use crate::hud::{self, HudLayout, HudState};
use crate::image_io;
use crate::input::InputMap;
use crate::menu::{MenuContext, MenuDoc};
use crate::scene_ui::SceneUi;
use crate::texture::{Image, TextureId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reloaded {
    Menu(String),
    Hud,
    Screens,
    Fonts,
    Strings,
    Image(String),
}

pub struct UiSystem {
    pub vfs: Arc<Vfs>,
    pub ctx: UiContext,
    pub menu_context: MenuContext,
    pub menus: HashMap<String, Arc<MenuDoc>>,
    pub hud_layout: HudLayout,
    pub hud_state: HudState,
    pub screens: ScreenLibrary,
    /// PNG images of screens (`UI/Images/...`), keyed by lower-case VFS path.
    pub images: HashMap<String, TextureId>,
    pub scene_ui: Option<SceneUi>,
    /// Archive that provides the shipped menu XML (`Scenes/HitmanBloodMoney.ZIP` unless changed).
    pub menu_archive: String,
    watched: HashMap<String, u64>,
}

fn hash_bytes(b: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    b.hash(&mut h);
    h.finish()
}

impl UiSystem {
    /// Creates the system and loads everything that does not need a scene: mod fonts, string tables, HUD layout,
    /// custom screens, user bindings.
    pub fn new(vfs: Arc<Vfs>) -> UiSystem {
        let mut s = UiSystem {
            vfs,
            ctx: UiContext::new(),
            menu_context: MenuContext::default(),
            menus: HashMap::new(),
            hud_layout: hud::default_layout(),
            hud_state: HudState::new(),
            screens: ScreenLibrary::default(),
            images: HashMap::new(),
            scene_ui: None,
            menu_archive: "Scenes/HitmanBloodMoney.ZIP".into(),
            watched: HashMap::new(),
        };
        s.reload_fonts();
        s.reload_strings();
        s.reload_hud();
        s.reload_screens();
        s.load_input_defaults();
        s
    }

    /// Loads the scene's fonts, strings and registers it for sprite lookups. Replaces a previous scene.
    pub fn load_scene_ui(&mut self, scene: &str) -> Result<(), crate::scene_ui::SceneUiError> {
        let mut sui = SceneUi::load(&self.vfs, scene)?;
        let fonts = sui.load_fonts(&mut self.ctx.textures);
        // game fonts first, then mod fonts replace same-named ones
        let mod_set = std::mem::take(&mut self.ctx.fonts);
        self.ctx.fonts = fonts;
        for name in mod_set.names() {
            if let Some(f) = mod_set.get(name) {
                self.ctx.fonts.insert(f.clone());
            }
        }
        self.reload_fonts();
        self.ctx.strings = crate::strings::Strings::new();
        sui.add_strings(&mut self.ctx.strings);
        self.reload_strings();
        self.scene_ui = Some(sui);
        Ok(())
    }

    /// Resolves the named sprite groups from the scene and registers them in the context.
    pub fn prefetch_sprites(&mut self, names: &[&str]) -> usize {
        let Some(sui) = self.scene_ui.as_mut() else { return 0 };
        let mut n = 0;
        for name in names {
            if let Some(sp) = sui.sprite(&mut self.ctx.textures, name).cloned() {
                self.ctx.add_sprite(name, sp);
                n += 1;
            } else {
                log::debug!("sprite '{name}' not found in scene");
            }
        }
        n
    }

    /// Registers a scene texture by TEX name as a full-size sprite (`Bitmaps/Screens/MainMenu/HMBMLogo_01`).
    pub fn add_scene_texture_sprite(&mut self, sprite_name: &str, tex_name: &str) -> bool {
        let Some(sui) = self.scene_ui.as_mut() else { return false };
        let Some(id) = sui.texture_by_name(&mut self.ctx.textures, tex_name) else { return false };
        let Some(img) = self.ctx.textures.get(id) else { return false };
        let size = [img.width as f32, img.height as f32];
        self.ctx.add_sprite(sprite_name, crate::sprite::Sprite::from_texture(sprite_name, id, size));
        true
    }

    fn menu_bytes(&mut self, name: &str) -> Option<Vec<u8>> {
        let path = format!("menu/{name}.xml");
        let info = self.vfs.stat(&path)?;
        let bytes = match info.source {
            Source::Archive(_) => self.vfs.open_in_archive(&self.menu_archive, &path).or_else(|_| self.vfs.open(&path)).ok()?,
            _ => self.vfs.open(&path).ok()?, // loose override
        };
        self.watched.insert(path, hash_bytes(&bytes));
        Some(bytes)
    }

    /// Loads (or returns the cached) menu document `name` (`frontend`, `ingame`, `briefing`, `default`, ...),
    /// applying `menu/<name>.add.xml` on top.
    pub fn load_menu(&mut self, name: &str) -> Option<Arc<MenuDoc>> {
        if let Some(d) = self.menus.get(&name.to_ascii_lowercase()) {
            return Some(d.clone());
        }
        let d = self.build_menu(name)?;
        self.menus.insert(name.to_ascii_lowercase(), d.clone());
        Some(d)
    }

    fn build_menu(&mut self, name: &str) -> Option<Arc<MenuDoc>> {
        let bytes = self.menu_bytes(name)?;
        let mut doc = MenuDoc::parse_bytes(&bytes, &format!("menu/{name}.xml"), &self.menu_context);
        let add_path = format!("menu/{name}.add.xml");
        if let Ok(add) = self.vfs.open(&add_path) {
            self.watched.insert(add_path.clone(), hash_bytes(&add));
            doc.merge(MenuDoc::parse_bytes(&add, &add_path, &self.menu_context));
        } else {
            self.watched.insert(add_path, 0);
        }
        for w in &doc.warnings {
            log::debug!("menu: {w}");
        }
        Some(Arc::new(doc))
    }

    pub fn reload_fonts(&mut self) {
        let names = self.ctx.fonts.load_mod_fonts(&self.vfs);
        self.ctx.fonts.register(&mut self.ctx.textures);
        for e in self.vfs.list("Fonts/*.fnt") {
            if let Ok(b) = self.vfs.open(&e.path) {
                self.watched.insert(e.path.clone(), hash_bytes(&b));
            }
        }
        if !names.is_empty() {
            log::info!("mod fonts: {names:?}");
        }
    }

    pub fn reload_strings(&mut self) {
        let n = self.ctx.strings.load_mod_tables(&self.vfs);
        self.track_prefix("Localisation/");
        if n > 0 {
            log::info!("{n} mod string table(s)");
        }
    }

    pub fn reload_hud(&mut self) {
        if let Some(l) = hud::load_layout(&self.vfs) {
            self.hud_layout = l;
        } else {
            self.hud_layout = hud::default_layout();
        }
        for p in ["UI/hud.ron", "UI/hud.json"] {
            if let Ok(b) = self.vfs.open(p) {
                self.watched.insert(p.to_string(), hash_bytes(&b));
            } else {
                self.watched.insert(p.to_string(), 0);
            }
        }
    }

    pub fn reload_screens(&mut self) {
        self.screens = ScreenLibrary::default();
        self.screens.load(&self.vfs);
        self.track_prefix("UI/Screens/");
        // images
        for e in self.vfs.list("UI/Images/*.png") {
            self.load_image(&e.path);
        }
    }

    /// Loads (or refreshes in place) a PNG from the VFS and returns its texture.
    pub fn load_image(&mut self, path: &str) -> Option<TextureId> {
        let bytes = self.vfs.open(path).ok()?;
        self.watched.insert(path.to_string(), hash_bytes(&bytes));
        let (w, h, rgba) = image_io::decode_png(&bytes).ok()?;
        let id = self.ctx.textures.add_or_replace(Image::new(w, h, rgba, format!("png:{path}")));
        self.images.insert(path.to_ascii_lowercase(), id);
        Some(id)
    }

    fn track_prefix(&mut self, prefix: &str) {
        for e in self.vfs.list(&format!("{prefix}**")) {
            if let Ok(b) = self.vfs.open(&e.path) {
                self.watched.insert(e.path.clone(), hash_bytes(&b));
            }
        }
    }

    fn load_input_defaults(&mut self) {
        if let Ok(b) = self.vfs.open("Config/input.ron") {
            if let Ok(m) = InputMap::from_ron(&crate::xml::decode_text(&b)) {
                self.ctx.input_map = m;
            }
        }
    }

    /// Loads the user's bindings from `path` (falls back to mod/default bindings when missing).
    pub fn load_user_bindings(&mut self, path: &std::path::Path) {
        if path.exists() {
            self.ctx.input_map = InputMap::load_or_default(path);
        }
    }

    /// Re-reads every watched file; returns what was reloaded. Cheap enough to call about once a second
    /// (it reads small text files; fonts/images are only re-decoded when they changed).
    pub fn poll_reload(&mut self) -> Vec<Reloaded> {
        let mut out = Vec::new();
        let paths: Vec<(String, u64)> = self.watched.iter().map(|(k, v)| (k.clone(), *v)).collect();
        let mut menus_changed: Vec<String> = Vec::new();
        let (mut hud_changed, mut screens_changed, mut fonts_changed, mut strings_changed) = (false, false, false, false);
        let mut images_changed: Vec<String> = Vec::new();
        for (path, old) in paths {
            let new = self.vfs.open(&path).map(|b| hash_bytes(&b)).unwrap_or(0);
            if new == old {
                continue;
            }
            let lp = path.to_ascii_lowercase();
            if let Some(rest) = lp.strip_prefix("menu/") {
                let n = rest.trim_end_matches(".xml").trim_end_matches(".add").to_string();
                if !menus_changed.contains(&n) {
                    menus_changed.push(n);
                }
            } else if lp.starts_with("ui/hud.") {
                hud_changed = true;
            } else if lp.starts_with("ui/screens/") {
                screens_changed = true;
            } else if lp.starts_with("ui/images/") {
                images_changed.push(path.clone());
            } else if lp.starts_with("fonts/") {
                fonts_changed = true;
            } else if lp.starts_with("localisation/") {
                strings_changed = true;
            }
            self.watched.insert(path, new);
        }
        for n in menus_changed {
            if self.menus.contains_key(&n) {
                match self.build_menu(&n) {
                    Some(d) => {
                        self.menus.insert(n.clone(), d);
                        out.push(Reloaded::Menu(n));
                    }
                    None => log::warn!("menu {n} vanished; keeping the previous document"),
                }
            }
        }
        if hud_changed {
            self.reload_hud();
            out.push(Reloaded::Hud);
        }
        if screens_changed {
            self.reload_screens();
            out.push(Reloaded::Screens);
        }
        if fonts_changed {
            self.reload_fonts();
            out.push(Reloaded::Fonts);
        }
        if strings_changed {
            self.reload_strings();
            out.push(Reloaded::Strings);
        }
        for p in images_changed {
            if self.load_image(&p).is_some() {
                out.push(Reloaded::Image(p));
            }
        }
        out
    }
}
