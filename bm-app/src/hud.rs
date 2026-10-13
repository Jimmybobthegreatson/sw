//! Game -> HUD glue: copies [`bm_game::UiState`] into `bm-ui`'s [`HudState`] and draws the overlay text
//! (objectives, mission banner, debug line).

use bm_game::{MissionStatus, UiState};
use bm_ui::{Action, Color, DrawList, HudState, Prompt, TextOptions, UiContext};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Default)]
pub struct HudBridge {
    seen_messages: HashSet<String>,
}

fn num(v: &Value) -> Option<f32> {
    v.as_f64().map(|x| x as f32)
}

impl HudBridge {
    /// `heading` is the camera heading in degrees (0 = +Z "north").
    pub fn update(&mut self, hud: &mut HudState, ui: &UiState, heading: f32, dt: f32) {
        for (k, v) in &ui.vars {
            if let Some(n) = num(v) {
                hud.set(k, n);
            } else if let Some(s) = v.as_str() {
                hud.set_text(k, s);
            }
        }
        hud.set("health_max", 100.0);
        hud.set("notoriety_max", 99.0);
        hud.set("suspicion", ui.alert_level as f32 / 3.0);
        hud.set("suspicion_max", 1.0);
        let weapon = ui.vars.get("weapon").and_then(|v| v.as_str()).unwrap_or("");
        hud.set("has_weapon", if weapon.is_empty() || weapon == "none" { 0.0 } else { 1.0 });
        hud.set_text("weapon_name", weapon.to_uppercase());
        hud.set("show_compass", 1.0);
        hud.set("heading", heading);
        hud.prompts = ui.prompts.iter().map(|p| Prompt { action: Action::UseAction, text: format!("%{p}") }).collect();
        // new messages become subtitles once
        let mut now = HashSet::new();
        for m in &ui.messages {
            now.insert(m.text.clone());
            if !self.seen_messages.contains(&m.text) {
                hud.say(None, &m.text, m.remaining.max(1.0));
            }
        }
        self.seen_messages = now;
        hud.update(dt);
    }
}

/// Objectives, mission result and the optional debug line, drawn in screen pixels.
pub fn draw_overlay(ctx: &UiContext, dl: &mut DrawList, screen: [f32; 2], ui: &UiState, debug: Option<&str>) {
    let s = (screen[1] / 720.0).max(0.5);
    let mut y = 56.0 * s;
    let x = 24.0 * s;
    let small = TextOptions { scale: s * 0.8, ..TextOptions::new("Text") };
    if !ui.mission_title.is_empty() {
        let l = ctx.layout_text(&ui.mission_title, &small);
        dl.text_shadow(&l, [x, y], Color::WHITE, [1.0, 1.0], Color([0, 0, 0, 200]));
        y += 18.0 * s;
    }
    for o in &ui.objectives {
        let mark = if o.done { "[x]" } else { "[ ]" };
        let col = if o.done { Color([150, 220, 150, 255]) } else if o.optional { Color([200, 200, 200, 255]) } else { Color::WHITE };
        let l = ctx.layout_text(&format!("{mark} {}", o.text), &small);
        dl.text_shadow(&l, [x, y], col, [1.0, 1.0], Color([0, 0, 0, 200]));
        y += 16.0 * s;
    }
    let banner = match ui.mission_status {
        MissionStatus::Complete => Some(("MISSION COMPLETE", Color([150, 230, 150, 255]))),
        MissionStatus::Failed => Some(("MISSION FAILED", Color([235, 80, 70, 255]))),
        _ => None,
    };
    if let Some((t, c)) = banner {
        let opts = TextOptions { scale: s * 1.6, ..TextOptions::new("Header") };
        let l = ctx.layout_text(t, &opts);
        let w = l.width;
        dl.text_shadow(&l, [(screen[0] - w) * 0.5, screen[1] * 0.35], c, [2.0, 2.0], Color([0, 0, 0, 220]));
    }
    if let Some(d) = debug {
        let l = ctx.layout_text(d, &small);
        dl.text_shadow(&l, [x, screen[1] - 20.0 * s], Color([255, 255, 120, 255]), [1.0, 1.0], Color([0, 0, 0, 220]));
    }
}
