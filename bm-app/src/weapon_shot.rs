//! `bm screenshot --weapon NAME|all`: every weapon model alone on a plain background from fixed angles (side, other side,
//! three-quarter view from above, top), the check for missing parts, overlapping variants and wrong materials.
//!
//! The weapon is drawn in its grip space (barrel along +Z, up = +Y, see `bm_game::props`) with the part set of its
//! `WeaponDef` (`parts` / `upgrades`) unless `--all-parts` or `--parts` say otherwise. One name gives a 2x2 view sheet of
//! the whole output size; several names (comma separated, or `all`) give contact sheets with one row of four views per weapon.

use crate::config::{Config, GameArgs, mount, resolve_scene};
use bm_game::data::GameData;
use bm_game::props::WeaponModels;
use bm_models::scene::{DrawItem, Scene};
use bm_render::{Camera, FrameParams, GpuContext, OffscreenTarget, Renderer, SceneGpu, UploadOptions};
use std::path::{Path, PathBuf};

/// Options of the weapon sheet mode.
pub struct WeaponShot {
    /// Weapon model group names, comma separated, case-insensitive substrings match; `all` = every weapon template.
    pub names: String,
    /// Part ids (`0x1f` or decimal, comma separated) overriding the weapon definition.
    pub parts: Option<String>,
    /// Draw every mesh of the model (no part filter).
    pub all_parts: bool,
    /// Upgrade names (keys of `WeaponDef::upgrade_parts`) to apply on top of the stock parts, comma separated.
    pub upgrades: Option<String>,
    /// Weapons per contact sheet in multi weapon mode.
    pub per_sheet: usize,
}

fn parse_id(s: &str) -> Option<u32> {
    let s = s.trim();
    match s.strip_prefix("0x") {
        Some(h) => u32::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

/// Camera positions around a centre, as offsets in units of the viewing distance: side, other side, above-behind, top.
const VIEWS: [[f32; 3]; 4] = [[1.0, 0.05, 0.0], [-1.0, 0.05, 0.0], [-0.55, 0.55, -0.62], [0.0, 1.0, -0.02]];

/// Renders the four views of one weapon into `tiles` (each `tw` x `th`, RGBA).
fn weapon_views(renderer: &mut Renderer, scene: &Scene, name: &str, node: usize, local: bm_models::Mat4, parts: &[u32], tw: u32, th: u32) -> Option<Vec<Vec<u8>>> {
    let model = scene.node_model(node)?;
    let mut opts = UploadOptions::default();
    if !parts.is_empty() {
        opts.part_filter_by_node.insert(node, parts.to_vec());
    }
    let mut remap: Vec<(u32, u32)> = scene.materials.stub_remap(name, model.meshes.iter().map(|m| m.material())).into_iter().collect();
    remap.sort_unstable();
    opts.material_remap_by_node.insert(node, remap);
    let items = [DrawItem { node, model: model.clone(), world: local }];
    let gpu = SceneGpu::upload(renderer, scene, &items, &opts);
    // frame what is drawn: meshes of the part set, without the glow quads (laser beams are metres long)
    let mut bb = bm_models::Aabb::EMPTY;
    for mesh in &model.meshes {
        let glow = mesh.flags & 0x800 != 0 && mesh.flags & 0x80 == 0;
        if mesh.is_lod0() && !glow && !mesh.indices.is_empty() && (parts.is_empty() || mesh.part == 0 || parts.contains(&mesh.part)) {
            bb = bb.union(&mesh.bbox());
        }
    }
    let bb = if bb.is_empty() { model.computed_bbox() } else { bb }.transformed(&local);
    let (c, size) = (bb.center(), bb.size());
    let ext = size[0].max(size[1]).max(size[2]).max(10.0);
    let target = OffscreenTarget::new(&renderer.device().clone(), tw, th, wgpu::TextureFormat::Rgba8UnormSrgb);
    let mut params = FrameParams::default();
    params.draw_sky = false;
    params.clear_color = [0.42, 0.45, 0.5, 1.0];
    params.shadows.enabled = false;
    params.fog_start = 1.0e6;
    params.fog_end = 2.0e6;
    let mut out = Vec::new();
    let aspect = tw as f32 / th as f32;
    for (vi, v) in VIEWS.into_iter().enumerate() {
        let mut cam = Camera { near: 1.0, ..Default::default() };
        // fit the longest extent: across the frame in the side views, along the vertical axis in the top view
        let wide = if vi == 3 { 1.0 } else { aspect.min(1.5) };
        let dist = ext * 0.62 / ((cam.fov_y * 0.5).tan() * wide) + size[0].max(size[1]) * 0.5;
        let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        cam.position = [c[0] + v[0] / n * dist, c[1] + v[1] / n * dist, c[2] + v[2] / n * dist];
        cam.look_at(c);
        out.push(target.render_rgba(renderer, Some(&gpu), &cam, &params));
    }
    Some(out)
}

fn blit(dst: &mut [u8], dw: usize, src: &[u8], sw: usize, sh: usize, x: usize, y: usize) {
    for row in 0..sh {
        let d = ((y + row) * dw + x) * 4;
        dst[d..d + sw * 4].copy_from_slice(&src[row * sw * 4..(row + 1) * sw * 4]);
    }
}

pub fn run(args: &GameArgs, cfg: &Config, level: &str, out: &Path, width: u32, height: u32, w: WeaponShot) -> Result<(), String> {
    let m = mount(args, cfg)?;
    let scene = Scene::load(&m.vfs, &resolve_scene(&m.vfs, level)).map_err(|e| format!("scene: {e}"))?;
    let wm = WeaponModels::build(&scene);
    let data = GameData::load(Some(&m.vfs));
    let all = wm.names();
    let wanted: Vec<String> = if w.names.eq_ignore_ascii_case("all") {
        all.clone()
    } else {
        let mut v = Vec::new();
        for n in w.names.split(',').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()) {
            let exact = all.iter().find(|a| **a == n);
            match exact.or_else(|| all.iter().find(|a| a.contains(&n))) {
                Some(a) => v.push(a.clone()),
                None => return Err(format!("no weapon model matches '{n}' (known: {})", all.join(", "))),
            }
        }
        v
    };
    let override_parts: Option<Vec<u32>> = w.parts.as_ref().map(|p| p.split(',').filter_map(parse_id).collect());
    let ctx = GpuContext::new_headless()?;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut renderer = Renderer::with_vfs(ctx.device.clone(), ctx.queue.clone(), format, Some(m.vfs.clone()));
    let multi = wanted.len() > 1;
    let (tw, th) = if multi { (width / 4, height / w.per_sheet.max(1) as u32) } else { (width / 2, height / 2) };
    let rows_per = if multi { w.per_sheet.max(1) } else { 1 };
    let mut written = Vec::new();
    for (sheet, group) in wanted.chunks(rows_per).enumerate() {
        let mut img = vec![0u8; (width * height * 4) as usize];
        for (row, name) in group.iter().enumerate() {
            let (node, local) = wm.get(name).ok_or("weapon vanished")?;
            let mut def = data.weapons.values().find(|d| d.model.eq_ignore_ascii_case(name)).cloned().unwrap_or_default();
            if let Some(u) = &w.upgrades {
                def.upgrades = u.split(',').map(|s| s.trim().to_string()).collect();
            }
            let parts = if w.all_parts {
                Vec::new()
            } else {
                override_parts.clone().unwrap_or_else(|| def.visible_parts())
            };
            println!("{name}: parts {}", if parts.is_empty() { "all".to_string() } else { parts.iter().map(|p| format!("{p:#x}")).collect::<Vec<_>>().join(",") });
            let Some(tiles) = weapon_views(&mut renderer, &scene, name, node, local, &parts, tw, th) else {
                println!("{name}: no model");
                continue;
            };
            for (i, t) in tiles.iter().enumerate() {
                let (cx, cy) = if multi { (i as u32 * tw, row as u32 * th) } else { ((i as u32 % 2) * tw, (i as u32 / 2) * th) };
                blit(&mut img, width as usize, t, tw as usize, th as usize, cx as usize, cy as usize);
            }
        }
        let path: PathBuf = if multi {
            let stem = out.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "weapons".into());
            out.with_file_name(format!("{stem}_{:02}.png", sheet + 1))
        } else {
            out.to_path_buf()
        };
        bm_render::offscreen::write_png(&path, width, height, &img).map_err(|e| format!("{}: {e}", path.display()))?;
        written.push(path);
    }
    for p in written {
        println!("wrote {}", p.display());
    }
    Ok(())
}
