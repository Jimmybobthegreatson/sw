//! `bm-ui` - menus, HUD, fonts, localisation and input bindings for the Glacier-1 runtime.
//!
//! Layering (everything except [`gpu`] is renderer-agnostic and unit-tested without a GPU):
//!
//! ```text
//! xml ──> menu (typed menu tree) ──> menu_view (layout, focus navigation, page stack, drawing)
//! strings (LOC lookup, markup) ─┐
//! font (bitmap fonts) ──> text (layout/wrap) ──> draw::DrawList (sprite/quad batcher) ──> gpu::UiRenderer (wgpu pass)
//! scene_ui (ZFONT/ZWINPIC/LOC of a scene) ─┘                                           └─> gpu::UiOffscreen (PNG)
//! hud (data-driven widgets)   custom (RON/JSON screens)   input (actions, bindings, user config)
//! system::UiSystem (VFS-backed loading + hot reload of all of the above)
//! ```
//!
//! See `docs/ui.md` for the data formats and the modding hooks.
//!
//! ```no_run
//! use std::sync::Arc;
//! use bm_ui::*;
//! let vfs = Arc::new(bm_vfs::Vfs::from_env().unwrap());
//! let mut ui = UiSystem::new(vfs);
//! ui.load_scene_ui("Scenes/HitmanBloodMoney").unwrap();            // game fonts + strings + sprites
//! let doc = ui.load_menu("frontend").unwrap();                       // menu/frontend.xml (+ mod overrides)
//! let mut nav = MenuNav::new(doc, "MainMenu");
//! let mut dl = DrawList::new();
//! dl.push_transform(Viewport::new(1280.0, 720.0, 1024.0, 768.0).transform());
//! nav.draw(&ui.ctx, &mut dl);                                        // then UiRenderer::render(.. &dl ..)
//! ```

pub mod context;
pub mod custom;
pub mod draw;
pub mod font;
pub mod gpu;
pub mod hud;
pub mod image_io;
pub mod input;
pub mod menu;
pub mod menu_view;
pub mod scene_ui;
pub mod sprite;
pub mod strings;
pub mod system;
pub mod text;
pub mod texture;
pub mod types;
pub mod xml;

pub use context::UiContext;
pub use custom::{ScreenAction, ScreenEvent, ScreenLibrary, ScreenStack, UiScreen};
pub use draw::{Blend, DrawCmd, DrawList, Transform, Vertex, Viewport};
pub use font::{BitmapFont, FontSet, Glyph};
pub use gpu::{Scene3d, UiOffscreen, UiRenderer};
pub use hud::{Hud, HudLayout, HudState, HudWidget, Prompt, Subtitle, WidgetKind};
pub use input::{Action, Binding, InputEvent, InputMap, InputState};
pub use menu::{Action as MenuAction, ItemKind, MenuContext, MenuDoc, Platform, Window};
pub use menu_view::{MenuEvent, MenuNav, Nav};
pub use scene_ui::SceneUi;
pub use sprite::{Sprite, SpritePart};
pub use strings::Strings;
pub use system::{Reloaded, UiSystem};
pub use text::{TextLayout, TextOptions};
pub use texture::{Image, TextureId, TextureStore};
pub use types::{Align, Color, Rect};
