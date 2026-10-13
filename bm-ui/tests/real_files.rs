//! Tests against the user's own Blood Money install (skipped when it is missing).

use std::sync::Arc;

use bm_ui::*;

fn vfs() -> Option<Arc<bm_vfs::Vfs>> {
    bm_vfs::game_dir_if_present()?;
    bm_vfs::Vfs::from_env().ok().map(Arc::new)
}

macro_rules! need_game {
    () => {
        match vfs() {
            Some(v) => v,
            None => {
                eprintln!("game dir not found: skipping");
                return;
            }
        }
    };
}

#[test]
fn every_shipped_menu_file_parses_and_lays_out() {
    let v = need_game!();
    let mut ui = UiSystem::new(v.clone());
    ui.load_scene_ui("Scenes/HitmanBloodMoney").unwrap();
    let mut total = 0;
    for name in ["default", "frontend", "online", "saveandc", "briefing", "ingame", "inventor", "postmiss", "premiss"] {
        let doc = ui.load_menu(name).unwrap_or_else(|| panic!("menu/{name}.xml"));
        assert!(!doc.windows.is_empty(), "{name} has no windows");
        total += doc.windows.len();
        for w in &doc.windows {
            // laying out every window must not panic and must keep items inside sane bounds
            let l = bm_ui::menu_view::layout_window(&doc, w, &ui.ctx, [1024.0, 768.0]);
            for it in &l.items {
                assert!(it.rect.x.is_finite() && it.rect.y.is_finite(), "{name}/{}", w.name);
            }
        }
    }
    assert!(total > 100, "only {total} windows");
}

#[test]
fn frontend_main_menu_structure() {
    let v = need_game!();
    let mut ui = UiSystem::new(v);
    let doc = ui.load_menu("frontend").unwrap();
    let main = doc.window("MainMenu").unwrap();
    assert_eq!((main.w, main.h), (1024.0, 768.0), "PC window template");
    assert!(!main.pop_on_back);
    let open: Vec<_> = main
        .walk()
        .into_iter()
        .filter_map(|i| match i.action() {
            Some(MenuAction::OpenWindow { window, .. }) => Some(window.clone()),
            _ => None,
        })
        .collect();
    assert!(open.contains(&"OptionMenu".to_string()) && open.contains(&"CreditsMenu".to_string()), "{open:?}");
    // the shipped file has duplicate attributes (`<Template Name=".." Name="..">`): tolerated with a warning
    assert!(doc.warnings.iter().any(|w| w.contains("duplicate attribute")), "{:?}", doc.warnings);
}

#[test]
fn scene_fonts_sprites_and_strings() {
    let v = need_game!();
    let mut ui = UiSystem::new(v);
    ui.load_scene_ui("Scenes/HitmanBloodMoney").unwrap();
    for name in ["Text", "Menu", "Header"] {
        let f = ui.ctx.fonts.get(name).unwrap_or_else(|| panic!("font {name}"));
        assert!(f.glyphs.len() > 150, "{name}: {} glyphs", f.glyphs.len());
        assert!(f.has('A') && f.has('z') && f.has('0'));
        assert!(f.pages.iter().all(|p| p.texture.is_some()));
        assert!(f.line_height > 10.0);
        // every glyph rectangle lies inside its page
        for g in f.glyphs.values() {
            let p = &f.pages[g.page as usize];
            assert!((g.x + g.w) as u32 <= p.width && (g.y + g.h) as u32 <= p.height, "{name} glyph out of page");
        }
    }
    assert_eq!(ui.ctx.strings.text("Frontend/StartGame"), "START GAME");
    assert_eq!(ui.ctx.strings.text("Buttons/Back"), "BACK {ZW_CANCEL}");
    assert_eq!(ui.prefetch_sprites(&["HitmanBackground"]), 1);
    let bg = ui.ctx.sprite("HitmanBackground").unwrap();
    assert_eq!(bg.size, [1024.0, 768.0]);
    assert!(bg.parts.len() >= 2);
    // the 9x11 scroll arrow is cut into four tiles
    ui.prefetch_sprites(&["ScrollDown#01"]);
    assert_eq!(ui.ctx.sprite("ScrollDown#01").unwrap().parts.len(), 4);
    // text with a button token resolves through the bindings
    let l = ui.ctx.layout_key("Buttons/Back", &TextOptions::new("Menu"));
    assert!(l.glyphs.len() > 8 && l.width > 50.0, "{:?}", l.line_texts());
    assert!(l.line_texts()[0].contains("[Esc]"), "{:?}", l.line_texts());
}

#[test]
fn osd_scene_has_hud_groups() {
    let v = need_game!();
    let mut ui = UiSystem::new(v);
    ui.load_scene_ui("Scenes/M01/M01_main").unwrap();
    let sui = ui.scene_ui.as_ref().unwrap();
    let names = sui.sprite_names();
    for n in ["compass", "framelarge", "focusbar", "tickoffmark"] {
        assert!(names.iter().any(|x| x == n), "missing OSD sprite {n}");
    }
}
