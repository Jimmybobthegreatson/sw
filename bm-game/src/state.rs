//! Per-frame output of the game for the presentation layer (`bm-app` feeds these to `bm-render` / `bm-ui`).
//! Plain data, no references into the simulation.

use crate::ai::AlertState;
use crate::anim::AnimState;
use crate::mission::{MissionStatus, ObjectiveState};
use crate::player::Camera;
use crate::world::Kind;
use bm_models::Mat4;
use bm_particles::RenderList;
use serde_json::Value;
use std::collections::BTreeMap;

/// A moving / posed scene object. Static level geometry is *not* listed: the renderer draws it from
/// `Scene::draw_list` once and only applies the overrides below.
#[derive(Clone, Debug)]
pub struct RenderEntity {
    pub id: u64,
    pub name: String,
    pub kind: Kind,
    /// Scene graph node whose model this entity moves (`SceneGpu::set_world(node, world)`).
    pub node: Option<usize>,
    pub prim_id: Option<u32>,
    /// Outfit value (variant mask of the skinned meshes this character wears, 1/5/9/13 ...; resolved against the
    /// model, 1 for the player). The renderer uploads exactly the meshes of this outfit (`UploadOptions::actor_outfits`).
    pub outfit: u8,
    /// The player wears somebody else's clothes: scene node of the donor model and the outfit value to draw. The renderer
    /// draws that model instead of the hero's (`skinning` is already computed for its skeleton).
    pub disguise: Option<(usize, u8)>,
    /// Entity -> world, native space (left-handed, Y up, cm).
    pub world: Mat4,
    pub visible: bool,
    /// Skinning matrices (`Skeleton::skinning_matrices`, one per bone) for characters (`SceneGpu::set_pose`).
    pub skinning: Option<Vec<Mat4>>,
    pub anim: AnimState,
    pub alert: Option<AlertState>,
    /// Damage flash 0..1 (tint hint).
    pub hit_flash: f32,
    pub alive: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct RenderLight {
    pub position: [f32; 3],
    /// Display space RGB (multiply with `intensity`).
    pub color: [f32; 3],
    /// Range end / start in cm (linear falloff in between).
    pub range: f32,
    pub range_start: f32,
    pub intensity: f32,
    pub spot: bool,
    /// Spot cone axis (world, unit) and `(cos outer, cos inner)`.
    pub direction: [f32; 3],
    pub cone: Option<(f32, f32)>,
}

#[derive(Clone, Debug)]
pub struct RenderState {
    pub time: f64,
    pub tick: u64,
    pub camera: Option<Camera>,
    pub entities: Vec<RenderEntity>,
    /// Scene nodes (with all their descendants) that must not be drawn (picked-up items ...).
    pub hidden_nodes: Vec<usize>,
    /// Weapon models visible this frame (held weapons, ground pickups), see [`crate::props`].
    pub props: Vec<crate::props::PropDraw>,
    /// Lights near the camera that are switched on, nearest first.
    pub lights: Vec<RenderLight>,
    pub particles: RenderList,
}

#[derive(Clone, Debug)]
pub struct UiMessage {
    pub text: String,
    pub remaining: f32,
}

/// HUD / menu state. `vars` use the variable names `bm-ui`'s HUD expects (`health`, `ammo_clip`, `notoriety`, ...).
#[derive(Clone, Debug, Default)]
pub struct UiState {
    pub vars: BTreeMap<String, Value>,
    /// Interaction prompts ("Open door").
    pub prompts: Vec<String>,
    pub messages: Vec<UiMessage>,
    pub objectives: Vec<ObjectiveState>,
    pub mission_status: MissionStatus,
    pub mission_title: String,
    /// Screens scripts asked for (`ui.show_screen`).
    pub screens: Vec<String>,
    /// 0 calm .. 3 combat, from the NPCs around the player.
    pub alert_level: u8,
    pub crosshair: bool,
    /// Room the player is in (decoded room map), e.g. `M01_Hacienda_Room_101`.
    pub room: Option<String>,
    /// Location group of that room (`M01_LOC_Ins_Hacienda_1stFloor.zip`).
    pub location: Option<String>,
    /// Security zone token of the room (`eZone3A_PATCH`).
    pub zone: Option<String>,
}
