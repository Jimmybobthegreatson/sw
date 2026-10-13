//! The original in-game HUD ("OSD"), laid out from the game's own code.
//!
//! Every position, size, colour and threshold here was read out of `HitmanBloodMoney.exe` (class `ZOSD`, `ZWeaponDisplay`;
//! `ZOSD::Layout` @ 0x6614e0, `ZOSD::Draw` @ 0x662490, compass draw @ 0x660680, compass scroll @ 0x661a20, stealth bar colour
//! @ 0x6601d0, weapon display draw @ 0x66e790) and the art comes from the scene's own `UserInterface/OSD/*` textures and
//! `ZWINPIC` sprites. Coordinates are native screen pixels (y down); the original draws everything at 1:1 pixels, so the
//! whole HUD is scaled by `screen_height / 600` here to keep the proportions of the 800x600 reference footage.

use bm_models::Scene;
use bm_ui::{Align, Blend, Color, DrawList, Image, Rect, TextOptions, TextureId, TextureStore, UiContext};

/// Reference height the pixel layout is drawn at.
pub const REF_H: f32 = 600.0;

/// Texture ids of the OSD art inside the [`TextureStore`].
#[derive(Clone, Default)]
pub struct OsdAssets {
    /// `ui_health_stealth_bars#health_bar` (red column, 8x38 at the top left of a 64x64 tile).
    pub health_bar: Option<TextureId>,
    /// `ui_health_stealth_bars#tricolor_bar_01` (white column, tinted green / yellow / red by the stealth value).
    pub tricolor_bar: Option<TextureId>,
    /// `ui_compass#CompassScale` (tick marks in the bottom rows).
    pub compass: Option<TextureId>,
    /// `Pointers_01_OSD_H3#Ingame Pointers` (crosshair 32x32 at the top left, red variants below).
    pub pointers: Option<TextureId>,
    /// `UI_all_weapons#...` class icons: rifle, smg, pistol, sniper, shotgun, close combat, item.
    pub rifle: Option<TextureId>,
    pub smg: Option<TextureId>,
    pub pistol: Option<TextureId>,
    pub sniper: Option<TextureId>,
    pub shotgun: Option<TextureId>,
    pub close_combat: Option<TextureId>,
    pub item: Option<TextureId>,
}

impl OsdAssets {
    /// Registers the scene's OSD textures. Textures that share a name are told apart by their order in the `.TEX` table,
    /// which is the order of the engine's own resource list (`health_bar` before `tricolor_bar_01`).
    pub fn load(scene: &Scene, store: &mut TextureStore) -> OsdAssets {
        let tex = scene.materials.tex();
        let mut by_name: std::collections::BTreeMap<String, Vec<u32>> = Default::default();
        for img in tex.images() {
            let n = img.name.to_ascii_lowercase();
            if n.contains("userinterface/osd/") || n.contains("bitmaps/osd/pointers") {
                by_name.entry(n).or_default().push(img.id);
            }
        }
        for v in by_name.values_mut() {
            v.sort_unstable();
        }
        let mut add = |id: u32, label: &str| -> Option<TextureId> {
            let img = scene.materials.image(id)?;
            let mut im = Image::new(img.width, img.height, img.rgba.clone(), label);
            im.nearest = true;
            Some(store.add(im))
        };
        let get = |name: &str, k: usize| by_name.get(name).and_then(|v| v.get(k)).copied();
        let weapons = by_name.get("userinterface/osd/ui_all_weapons").cloned().unwrap_or_default();
        // sheet order in the .TEX table: M4 rifle, MP5, pistol, WA2000 sniper, SPAS12 shotgun, then the fist and the open hand
        let w = |k: usize| weapons.get(k).copied();
        OsdAssets {
            health_bar: get("userinterface/osd/ui_health_stealth_bars", 0).and_then(|i| add(i, "osd health_bar")),
            tricolor_bar: get("userinterface/osd/ui_health_stealth_bars", 1).and_then(|i| add(i, "osd tricolor_bar")),
            compass: get("userinterface/osd/ui_compass", 0).and_then(|i| add(i, "osd compass")),
            pointers: get("bitmaps/osd/pointers_01_osd_h3", 0).and_then(|i| add(i, "osd pointers")),
            rifle: w(0).and_then(|i| add(i, "osd rifle")),
            smg: w(1).and_then(|i| add(i, "osd smg")),
            pistol: w(2).and_then(|i| add(i, "osd pistol")),
            sniper: w(3).and_then(|i| add(i, "osd sniper")),
            shotgun: w(4).and_then(|i| add(i, "osd shotgun")),
            close_combat: w(5).and_then(|i| add(i, "osd close combat")),
            item: w(6).and_then(|i| add(i, "osd item")),
        }
    }
}

/// Icon class of the selected item.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WeaponClass {
    Rifle,
    Smg,
    Pistol,
    Sniper,
    Shotgun,
    CloseCombat,
    Item,
}

/// What the HUD shows this frame.
pub struct NativeHud<'a> {
    /// Health 0..1.
    pub health: f32,
    /// The native stealth value 0..100 (`ZOSD+0x18c`): green up to 50, yellow up to 75, red above.
    pub stealth: f32,
    /// Camera heading in degrees (0 = north, clockwise).
    pub heading: f32,
    /// Display name of the selected item, empty = none.
    pub weapon: &'a str,
    pub class: WeaponClass,
    /// "clip / reserve" line of the weapon display, empty = hidden.
    pub bullets: String,
    /// The pointer is shown (item drawn / aiming).
    pub pointer: bool,
    /// An interaction is available.
    pub can_act: bool,
    /// The item selector panel: `(name, state word, description)`, `None` = closed.
    pub panel: Option<ItemPanel<'a>>,
}

/// The item selector shown while cycling through the inventory.
pub struct ItemPanel<'a> {
    pub name: &'a str,
    /// "HOLSTER" while the item is drawn.
    pub state: &'a str,
    pub description: &'a str,
    /// 0..1 fade in / out.
    pub alpha: f32,
}

const WHITE: Color = Color([255, 255, 255, 255]);
const GREEN: Color = Color([0x00, 0xae, 0x09, 255]);
const YELLOW: Color = Color([0xe9, 0xc6, 0x16, 255]);
const RED: Color = Color([0xda, 0x00, 0x00, 255]);

fn text(ctx: &UiContext, dl: &mut DrawList, s: &str, font: &str, at: [f32; 2], align: Align, color: Color) {
    let o = TextOptions::new(font);
    let l = ctx.layout_text(s, &o);
    let x = match align {
        Align::Left => at[0],
        Align::Center => at[0] - l.width * 0.5,
        Align::Right => at[0] - l.width,
    };
    dl.text_outline(&l, [x, at[1]], color, Color([0, 0, 0, 220]), 1.0);
}

/// Draws the HUD. `screen` is the window size in logical pixels.
pub fn draw(ctx: &UiContext, assets: &OsdAssets, dl: &mut DrawList, screen: [f32; 2], h: &NativeHud) {
    let s = screen[1] / REF_H;
    let (w, ht) = (screen[0] / s, REF_H);
    dl.push_transform(bm_ui::Transform { scale: s, offset: [0.0, 0.0] });
    let quad = |dl: &mut DrawList, tex: Option<TextureId>, r: Rect, uv: [f32; 4], c: Color| {
        if let Some(t) = tex {
            dl.quad(t, r, uv, c, Blend::Alpha);
        }
    };

    // ---- health frame (ZOSD+0x3a0: (48, H-101)) with the health bar and the stealth bar inside
    if let Some(frame) = ctx.sprite("healthframe") {
        frame.draw(dl, [48.0, ht - 101.0], None, WHITE);
    }
    // bars are 8x38 columns of their 64x64 tiles; the quad grows upwards from y = H-58.5
    let bottom = ht - 58.5;
    let col = |frac: f32| -> ([f32; 4], f32) {
        let hgt = 38.0 * frac.clamp(0.0, 1.0);
        ([0.0, (38.0 - hgt) / 64.0, 8.0 / 64.0, 38.0 / 64.0], hgt)
    };
    let (uv, hgt) = col(h.health);
    quad(dl, assets.health_bar, Rect::new(61.5, bottom - hgt, 8.0, hgt), uv, WHITE);
    let stealth_frac = if h.stealth <= 100.0 { h.stealth * 0.009 } else { 0.9 };
    let tint = if h.stealth <= 50.0 { GREEN } else if h.stealth <= 75.0 { YELLOW } else { RED };
    let (uv, hgt) = col(stealth_frac);
    quad(dl, assets.tricolor_bar, Rect::new(86.5, bottom - hgt, 8.0, hgt), uv, tint);

    // ---- compass (ZOSD::Layout +0x144.. : scale at x = W-129.5, letters 32 px apart starting at W-96, marker at W-100)
    let base = w - 96.0;
    let shift = (h.heading.rem_euclid(360.0) / 360.0 * 128.0).floor();
    // scale strip: the bottom rows of the 64x64 tile, 65 texels wide, scrolled by the heading
    let f = shift / 64.0;
    quad(dl, assets.compass, Rect::new(w - 129.5, 35.5 - 8.0, 65.0, 8.0), [f, 56.0 / 64.0, f + 65.0 / 64.0, 1.0], WHITE);
    if let Some(marker) = ctx.sprite("compassmarker") {
        marker.draw(dl, [w - 100.0, 35.0], None, WHITE);
    }
    for (i, ch) in ["N", "E", "S", "W"].iter().enumerate() {
        let mut x = base + i as f32 * 32.0 - shift;
        if x < base - 64.0 {
            x += 128.0;
        } else if x > base + 64.0 {
            x -= 128.0;
        }
        // full alpha within one letter of the centre, fading to nothing at two
        let d = (x - base).abs();
        let a = if d <= 32.0 { 1.0 } else { (1.0 - (d - 32.0) / 32.0).clamp(0.0, 1.0) };
        if a > 0.01 {
            text(ctx, dl, ch, "Menu", [x, 17.0 - 6.0], Align::Center, Color([255, 255, 255, (a * 254.0) as u8]));
        }
    }

    // ---- weapon display (ZOSD+0x80: (W-212, H-99)); FrameLarge at (x0-16, y0-2), icon top-centre at (x0+104, y0)
    if !h.weapon.is_empty() {
        let (x0, y0) = (w - 212.0, ht - 99.0);
        if let Some(frame) = ctx.sprite("framelarge") {
            frame.draw(dl, [x0 - 16.0, y0 - 2.0], None, WHITE);
        }
        let icon = match h.class {
            WeaponClass::Rifle => assets.rifle,
            WeaponClass::Smg => assets.smg,
            WeaponClass::Pistol => assets.pistol,
            WeaponClass::Sniper => assets.sniper,
            WeaponClass::Shotgun => assets.shotgun,
            WeaponClass::CloseCombat => assets.close_combat,
            WeaponClass::Item => assets.item,
        };
        quad(dl, icon, Rect::new(x0 + 104.0 - 64.0, y0, 128.0, 64.0), [0.0, 0.0, 1.0, 1.0], WHITE);
        // name right-aligned to the frame's right edge (measured on a capture of the original at 800x600); the BulletInfo
        // node sits 25 px below the name line (scene nodes WeaponDisplay/Text and WeaponDisplay/BulletInfo)
        text(ctx, dl, &h.weapon.to_uppercase(), "Menu", [x0 + 161.0, y0 + 46.0], Align::Right, WHITE);
        if !h.bullets.is_empty() {
            text(ctx, dl, &h.bullets, "Menu", [x0 + 104.0, y0 + 69.0], Align::Center, WHITE);
        }
    }

    // ---- pointer (ZOSD+0x6c: screen centre minus 16), white 32x32 tile; the red one is for enemies under the pointer
    if h.pointer {
        quad(dl, assets.pointers, Rect::new(w * 0.5 - 16.0 + 0.5, ht * 0.5 - 16.0 + 0.5, 32.0, 32.0), [0.0, 0.0, 0.5, 0.5], WHITE);
    }

    // ---- action key hints G / SPC / E. Pixel boxes measured on a capture of the original game at 800x600 (the three
    // `DropDisplay` / `ActionMenu` / `PickupMenu` groups; the code that positions them could not be traced statically).
    let hint = |dl: &mut DrawList, label: &str, r: Rect, lit: bool, _big: bool| {
        // bevelled grey key cap: dark outline, light top / left edge, shaded bottom / right edge (brighter while available)
        let (fill, hi, lo, txt) = if lit { (120u8, 168u8, 74u8, 232u8) } else { (70, 104, 46, 200) };
        dl.rect(r, Color([24, 24, 24, 235]));
        let inner = Rect::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, r.h - 2.0);
        dl.rect(inner, Color([fill, fill, fill - 4, 235]));
        dl.rect(Rect::new(inner.x, inner.y, inner.w, 1.0), Color([hi, hi, hi, 235]));
        dl.rect(Rect::new(inner.x, inner.y, 1.0, inner.h), Color([hi, hi, hi, 235]));
        dl.rect(Rect::new(inner.x, inner.bottom() - 1.0, inner.w, 1.0), Color([lo, lo, lo, 235]));
        dl.rect(Rect::new(inner.right() - 1.0, inner.y, 1.0, inner.h), Color([lo, lo, lo, 235]));
        let l = ctx.layout_text(label, &TextOptions { scale: 0.85, ..TextOptions::new("Header") });
        dl.text(&l, [r.x + (r.w - l.width) * 0.5, r.y + (r.h - l.height) * 0.5], Color([txt, txt, txt, 255]));
    };
    hint(dl, "G", Rect::new(49.0, 23.0, 16.0, 16.0), h.can_act, false);
    hint(dl, "SPC", Rect::new(77.0, 49.0, 30.0, 15.0), h.can_act, true);
    hint(dl, "E", Rect::new(49.0, 76.0, 16.0, 15.0), h.can_act, false);
    dl.pop_transform();

    if let Some(p) = &h.panel {
        draw_panel(ctx, dl, w, ht, p);
    }
    dl.pop_transform();
}

/// The item selector, measured on a capture of the original game at 800x600 (opened by holding the right mouse button):
/// a 370 px wide panel with a grey header bar (arrows 12 px outside both ends, a small square, the name), a near-black
/// description area and a grey footer bar; the picture behind it is blurred and darkened (only darkened here).
fn draw_panel(ctx: &UiContext, dl: &mut DrawList, w: f32, ht: f32, p: &ItemPanel) {
    let a = (p.alpha.clamp(0.0, 1.0) * 255.0) as u8;
    let al = |c: [u8; 4]| Color([c[0], c[1], c[2], ((c[3] as u32 * a as u32) / 255) as u8]);
    dl.rect(Rect::new(0.0, 0.0, w, ht), al([0, 0, 0, 150]));
    let (x, bw) = (w * 0.5 - 185.0, 370.0);
    let (hy, hh, by, bh, fy, fh) = (426.0, 22.0, 448.0, 100.0, 548.0, 22.0);
    // header: lighter at the top, 1 px highlight on the top edge and a darker bottom edge
    dl.gradient_v(Rect::new(x, hy, bw, hh), al([107, 107, 107, 255]), al([82, 82, 82, 255]));
    dl.rect(Rect::new(x, hy - 1.0, bw, 1.0), al([119, 118, 118, 255]));
    dl.rect(Rect::new(x, hy + hh - 1.0, bw, 1.0), al([63, 63, 63, 255]));
    dl.rect(Rect::new(x, by, bw, bh), al([7, 7, 7, 255]));
    dl.rect(Rect::new(x, by, 2.0, bh), al([46, 46, 46, 255]));
    dl.rect(Rect::new(x, fy, bw, fh), al([39, 39, 39, 255]));
    // arrows: 13 px wide, 16 px tall, tips pointing away from the panel
    let cy = hy + hh * 0.5;
    for r in 0..13 {
        let half = (r + 1) as f32 * 0.62;
        dl.rect(Rect::new(x - 16.0 + r as f32, cy - half, 1.0, half * 2.0), al([162, 162, 162, 255]));
        dl.rect(Rect::new(x + bw + 16.0 - r as f32 - 1.0, cy - half, 1.0, half * 2.0), al([162, 162, 162, 255]));
    }
    dl.rect(Rect::new(x + 8.0, cy - 3.5, 7.0, 7.0), al([190, 190, 190, 255]));
    let ln = ctx.layout_text(&p.name.to_uppercase(), &TextOptions::new("Menu"));
    dl.text(&ln, [x + 23.0, hy + (hh - ln.height) * 0.5], Color([255, 255, 255, a]));
    if !p.state.is_empty() {
        let ls = ctx.layout_text(&p.state.to_uppercase(), &TextOptions::new("Menu"));
        dl.text(&ls, [x + 23.0 + ln.width + 8.0, hy + (hh - ls.height) * 0.5], Color([150, 150, 150, a]));
    }
    let body = TextOptions { max_width: Some(bw - 20.0), ..TextOptions::new("Text") };
    let ld = ctx.layout_text(p.description, &body);
    dl.text(&ld, [x + 14.0, by + 6.0], Color([235, 235, 235, a]));
}
