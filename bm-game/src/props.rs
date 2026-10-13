//! Weapon models in the world: held by the player / armed NPCs and lying on the ground as pickups.
//!
//! The game ships every weapon as a node group `AllLevels/equipment.zip/AllLevels/weapons.zip/Weapons/<class>/<name>`
//! (e.g. `Custom/Custom_Pistol`) with a `Ground01` link node carrying the model. The group's origin is the weapon's grip
//! space (barrel along +Z, `PosBox_MuzzleExit` marks the muzzle), so a weapon is drawn at
//! `hand attacher bone * grip offset * (group -> Ground01 local)`; a pickup at `pickup node * (group -> Ground01 local)`.
//!
//! The renderer cannot instance a node twice, so [`crate::Game::prop_slots`] lists one *slot* per (owner, weapon) that
//! `bm-app` uploads as an extra item; [`crate::RenderState::props`] then names the slots that are visible this frame.

use bm_models::scene::{NodeKind, Scene};
use bm_models::{Mat4, Pose, Skeleton};
use std::collections::HashMap;

/// A drawable weapon model the renderer must create once (key is stable for the whole level).
#[derive(Clone, Debug)]
pub struct PropSlot {
    pub key: String,
    /// Scene node (`Ground01` link) whose model is drawn.
    pub node: usize,
    /// Weapon model group name (`Custom_Pistol`, as in `WeaponDef::model`, or a pickup's item name).
    pub model: String,
    /// Part ids of the model to draw (`WeaponDef::visible_parts`); empty = every mesh. Slots that share a `node` share the
    /// part set the renderer uploads (`UploadOptions::part_filter_by_node`).
    pub parts: Vec<u32>,
}

/// A slot that is visible this frame, with its model -> world matrix (the node's own transform is already included).
#[derive(Clone, Debug)]
pub struct PropDraw {
    pub key: String,
    pub world: Mat4,
}

/// Weapon template groups of a scene by lower-case group name (`custom_pistol`, `smg_mp5_01`).
#[derive(Default)]
pub struct WeaponModels {
    by_name: HashMap<String, (usize, Mat4)>,
    /// `PosBox_MuzzleExit` of each template in grip space (cm).
    muzzle: HashMap<String, [f32; 3]>,
}

impl WeaponModels {
    pub fn build(scene: &Scene) -> WeaponModels {
        let mut by_name: HashMap<String, (usize, Mat4)> = HashMap::new();
        let mut muzzle: HashMap<String, [f32; 3]> = HashMap::new();
        for n in &scene.nodes {
            if n.kind != NodeKind::Link || n.prim_id.is_none() || !n.name.to_ascii_lowercase().starts_with("ground") {
                continue;
            }
            let Some(parent) = n.parent.map(|p| &scene.nodes[p]) else { continue };
            let path = scene.path(n.index).to_ascii_lowercase();
            if !path.contains("weapons.zip/weapons/") || parent.name.to_ascii_lowercase().ends_with("_pickup") {
                continue;
            }
            // group -> model link (the link is placed relative to the weapon's grip space)
            let local = parent.world.inverse_affine().mul(&n.world);
            let key = parent.name.to_ascii_lowercase();
            if let Some(m) = scene.nodes.iter().find(|c| c.parent == Some(parent.index) && c.name == "PosBox_MuzzleExit") {
                muzzle.entry(key.clone()).or_insert_with(|| parent.world.inverse_affine().transform_point(m.world.translation()));
            }
            by_name.entry(key).or_insert((n.index, local));
        }
        WeaponModels { by_name, muzzle }
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// Muzzle position (`PosBox_MuzzleExit`) of a weapon template in its grip space, if it has one (melee weapons do not).
    pub fn muzzle(&self, name: &str) -> Option<[f32; 3]> {
        self.muzzle.get(&name.to_ascii_lowercase()).copied()
    }

    /// Lower-case group names of all weapon templates, sorted.
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.by_name.keys().cloned().collect();
        v.sort();
        v
    }

    /// `(node, group -> model matrix)` of a weapon template by group name (case-insensitive).
    pub fn get(&self, name: &str) -> Option<(usize, Mat4)> {
        self.by_name.get(&name.to_ascii_lowercase()).copied()
    }
}

/// Model-space matrix of the right hand attacher of a posed skeleton.
pub fn hand_matrix(sk: &Skeleton, pose: &Pose) -> Option<Mat4> {
    let i = sk.bone_index("RIGHT_HAND_ATTACHER").or_else(|| sk.bone_index("RIGHT_HAND"))?;
    sk.world_matrices(pose).get(i).copied()
}

/// Rotation about an axis (0 = X, 1 = Y, 2 = Z) in degrees, translation `t` (cm): the weapon's grip offset in the hand frame.
pub fn grip(rot_deg: [f32; 3], t: [f32; 3]) -> Mat4 {
    let r = |axis: usize, a: f32| {
        let (s, c) = a.to_radians().sin_cos();
        match axis {
            0 => Mat4([[1.0, 0.0, 0.0, 0.0], [0.0, c, s, 0.0], [0.0, -s, c, 0.0], [0.0, 0.0, 0.0, 1.0]]),
            1 => Mat4([[c, 0.0, -s, 0.0], [0.0, 1.0, 0.0, 0.0], [s, 0.0, c, 0.0], [0.0, 0.0, 0.0, 1.0]]),
            _ => Mat4([[c, s, 0.0, 0.0], [-s, c, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]),
        }
    };
    let mut m = r(1, rot_deg[1]).mul(&r(0, rot_deg[0])).mul(&r(2, rot_deg[2]));
    m.0[3] = [t[0], t[1], t[2], 1.0];
    m
}
