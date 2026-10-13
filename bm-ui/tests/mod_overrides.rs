//! Loose-file overrides + hot reload through the VFS, using a throw-away mod directory (no game files needed).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bm_ui::*;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("bm-ui-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn write(root: &Path, rel: &str, content: &[u8]) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

const MENU_V1: &str = r#"<Windows>
  <ColorSetDefinition Name="Default" NormalColor="0xffffffff" FocusColor="0xffffffff" />
  <Window Name="MainMenu" W="1024" H="768" AlignH="Center" AlignV="Center">
    <GuiElement Type="OpenWindow" Text="%One" Window="Other" X="100" Y="100" />
  </Window>
  <Window Name="Other" W="1024" H="768"><GuiElement Type="CloseWindow" Text="%Back" X="10" Y="10" /></Window>
</Windows>"#;
const MENU_V2: &str = r#"<Windows><Window Name="MainMenu" W="1024" H="768"><GuiElement Type="TextBlock" Text="%Two" X="1" Y="1" /><GuiElement Type="TextBlock" Text="%Three" X="1" Y="30" /></Window></Windows>"#;
const MENU_ADD: &str = r#"<Windows><Window Name="Extra" W="10" H="10"><GuiElement Type="TextBlock" Text="%Added" /></Window></Windows>"#;

fn fnt_font() -> (String, Vec<u8>) {
    // 16x16 page, one glyph 'A' (4x6) + space
    let fnt = "info face=\"ModFont\" size=8\ncommon lineHeight=10 base=8 pages=1\npage id=0 file=\"ModFont_0.png\"\nchars count=2\nchar id=32 x=0 y=0 width=0 height=0 xoffset=0 yoffset=0 xadvance=3 page=0\nchar id=65 x=0 y=0 width=4 height=6 xoffset=0 yoffset=2 xadvance=5 page=0\n".to_string();
    let png = image_io::encode_png(16, 16, &vec![255u8; 16 * 16 * 4]).unwrap();
    (fnt, png)
}

#[test]
fn mod_files_override_and_hot_reload() {
    let root = temp_dir("mod");
    write(&root, "menu/frontend.xml", MENU_V1.as_bytes());
    write(&root, "menu/frontend.add.xml", MENU_ADD.as_bytes());
    let (fnt, png) = fnt_font();
    write(&root, "Fonts/ModFont.fnt", fnt.as_bytes());
    write(&root, "Fonts/ModFont_0.png", &png);
    write(&root, "Localisation/en.json", br#"{"Frontend/StartGame": "NEW GAME", "AllLevels/Interface/Dialog/X": "modded"}"#);
    write(&root, "UI/hud.ron", br#"(name: "modhud", widgets: [(id: "t", kind: Text(text: "%hi"))])"#);
    write(&root, "UI/Screens/s.ron", br##"(name: "s", elements: [(id: "p", kind: Panel(color: "#fff"))])"##);
    write(&root, "UI/Images/pic.png", &png);

    let vfs = Arc::new(bm_vfs::Vfs::new(&root).unwrap());
    let mut ui = UiSystem::new(vfs);

    // fonts
    let f = ui.ctx.fonts.get("ModFont").expect("mod font");
    assert_eq!(f.glyphs.len(), 2);
    assert!(f.pages[0].texture.is_some(), "page registered in the texture store");
    // strings (flat table, relative + absolute keys)
    assert_eq!(ui.ctx.strings.text("Frontend/StartGame"), "NEW GAME");
    assert_eq!(ui.ctx.strings.text("Dialog/X"), "modded");
    // hud + screens + images
    assert_eq!(ui.hud_layout.name, "modhud");
    assert!(ui.screens.get("s").is_some());
    assert!(ui.images.contains_key("ui/images/pic.png"));
    // menu: replaced doc + .add.xml merged
    let doc = ui.load_menu("frontend").unwrap();
    assert_eq!(doc.window("MainMenu").unwrap().items.len(), 1);
    assert!(doc.window("Extra").is_some());

    // nothing changed -> nothing reloaded
    assert!(ui.poll_reload().is_empty());

    // edit the files in place
    write(&root, "menu/frontend.xml", MENU_V2.as_bytes());
    write(&root, "Localisation/en.json", br#"{"Frontend/StartGame": "PLAY"}"#);
    write(&root, "UI/hud.ron", br#"(name: "modhud2", widgets: [])"#);
    let (fnt2, _) = fnt_font();
    write(&root, "Fonts/ModFont.fnt", fnt2.replace("xadvance=5", "xadvance=9").as_bytes());
    write(&root, "UI/Screens/s.ron", br#"(name: "s2", elements: [])"#);
    let r = ui.poll_reload();
    assert!(r.contains(&Reloaded::Menu("frontend".into())), "{r:?}");
    assert!(r.contains(&Reloaded::Strings) && r.contains(&Reloaded::Hud) && r.contains(&Reloaded::Fonts) && r.contains(&Reloaded::Screens), "{r:?}");
    assert_eq!(ui.hud_layout.name, "modhud2");
    assert_eq!(ui.ctx.strings.text("Frontend/StartGame"), "PLAY");
    assert_eq!(ui.ctx.fonts.get("ModFont").unwrap().glyph('A').unwrap().advance, 9);
    assert!(ui.screens.get("s2").is_some() && ui.screens.get("s").is_none());
    let doc = ui.load_menu("frontend").unwrap();
    assert_eq!(doc.window("MainMenu").unwrap().items.len(), 2);

    // a broken HUD edit falls back to the builtin layout instead of crashing
    write(&root, "UI/hud.ron", b"((((");
    ui.poll_reload();
    assert_eq!(ui.hud_layout.name, "default");
    fs::remove_dir_all(&root).ok();
}

#[test]
fn input_user_config_round_trip_via_system() {
    let root = temp_dir("input");
    let vfs = Arc::new(bm_vfs::Vfs::new(&root).unwrap());
    let mut ui = UiSystem::new(vfs);
    let path = root.join("user").join("input.ron");
    ui.load_user_bindings(&path); // missing -> defaults retained
    assert_eq!(ui.ctx.input_map, InputMap::defaults());
    let mut m = ui.ctx.input_map.clone();
    m.rebind(Action::Map, 0, Binding::key("KeyN"));
    m.save(&path).unwrap();
    ui.load_user_bindings(&path);
    assert_eq!(ui.ctx.input_map.get(Action::Map)[0], Binding::key("KeyN"));
    fs::remove_dir_all(&root).ok();
}
