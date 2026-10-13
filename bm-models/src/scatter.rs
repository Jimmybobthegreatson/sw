//! `ZScatterObj` (type `0x2000ce`, PRM object word 0 `0x90109`): procedural vegetation clumps.
//!
//! A scatter node owns no vertex data. Its PRM object describes a *volume* and a table of random numbers; the engine
//! (`ScatterBillboard.fx`) turns them into camera facing sprites of the material's atlas texture. Decoded layout (**M**, verified
//! by the table sizes / bounding boxes of every M01 scatter node and visually on the foliage of M01):
//!
//! ```text
//! w4 >> 16   material instance + 1: a `ScatterMaterial` with floats ScatterWidth / ScatterHeight / ScatterRandomHeight (cm)
//!            and a diffuse atlas texture of square frames laid out in one row (columns = width / height)
//! w11        PRM chunk: 8 pattern records (n_a, n_b, first instance, 0) with n_a + n_b = instances per pattern, then the
//!            instance table at +0x80: one 4 x f32 record per instance = (a, b, k, c); a, b, c are random numbers in 0..1
//!            and k is constant per object (0.05 .. 0.08)
//! w12        size of that chunk (bytes); w14 = instance count (120 / 240 / 360 = 8 patterns x 15 / 30 / 45)
//! w13        PRM chunk with one type byte per instance (0..7; the atlas frame is `type % columns`)
//! w15        100.0 (cell size?), w16..w21 = volume centre xyz and half extent xyz in node space (= the model header box)
//! w23..w28   two colours (1,1,1)/(0.29,0.29,0.29): tint / far colour, w22 a seed-like constant
//! ```
//!
//! Interpretation used here (the original instancing code is native): instance `i` stands at `x = min.x + a * size.x`,
//! `z = min.z + b * size.z`; its height is the highest upward facing level geometry (names containing ground / landscape / grass /
//! rock ...) inside the volume's vertical slab at that point - volumes without ground inside (tree crowns hanging 18 m up) use
//! `y = min.y + c * size.y`. `c` also drives yaw and the random height. Every instance is two crossed, double sided quads.

use std::sync::Arc;

use bm_formats::prm::Prm;

use crate::material::MaterialLibrary;
use crate::math::Aabb;
use crate::mesh::{Mesh, Model, SubMesh, VertexData, VertexLayout};
use crate::scene::Scene;

pub const SCATTER_TAG: u32 = 0x0009_0109;

/// Words of a scatter object, `None` when the model is not a scatter model.
pub fn scatter_object(prm: &Prm, prim_id: u32) -> Option<(Vec<u32>, Aabb, u32)> {
    let h = prm.model(prim_id as usize)?;
    let objs = prm.chunk_words(h.objects_chunk as usize)?;
    for &oc in objs.iter().take(h.object_count as usize) {
        let w = prm.chunk_words(oc as usize)?;
        if w.first() == Some(&SCATTER_TAG) && w.len() >= 30 {
            return Some((w, Aabb { min: h.bbox_min, max: h.bbox_max }, oc));
        }
    }
    None
}

fn hash01(i: u32, salt: u32) -> f32 {
    let mut x = i.wrapping_mul(0x9e37_79b1) ^ salt.wrapping_mul(0x85eb_ca6b);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2c1b_3c6d);
    x ^= x >> 12;
    x = x.wrapping_mul(0x297a_2d39);
    x ^= x >> 15;
    (x & 0xffffff) as f32 / 16777216.0
}

fn is_ground_name(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    ["ground", "landscape", "terrain", "hill", "field", "grass", "rock", "cliff", "slope", "mountain", "soil", "jungle"].iter().any(|k| l.contains(k))
}

struct Tri {
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
}

/// Highest point of `tris` above (x, z), if any.
fn ground_at(tris: &[Tri], x: f32, z: f32) -> Option<f32> {
    let mut best: Option<f32> = None;
    for t in tris {
        let den = (t.b[2] - t.c[2]) * (t.a[0] - t.c[0]) + (t.c[0] - t.b[0]) * (t.a[2] - t.c[2]);
        if den.abs() < 1e-6 {
            continue;
        }
        let l0 = ((t.b[2] - t.c[2]) * (x - t.c[0]) + (t.c[0] - t.b[0]) * (z - t.c[2])) / den;
        let l1 = ((t.c[2] - t.a[2]) * (x - t.c[0]) + (t.a[0] - t.c[0]) * (z - t.c[2])) / den;
        let l2 = 1.0 - l0 - l1;
        if l0 < 0.0 || l1 < 0.0 || l2 < 0.0 {
            continue;
        }
        let y = l0 * t.a[1] + l1 * t.b[1] + l2 * t.c[1];
        if best.is_none_or(|b| y > b) {
            best = Some(y);
        }
    }
    best
}

fn atlas_columns(mats: &MaterialLibrary, material: u32) -> u32 {
    mats.desc(material)
        .and_then(|d| d.diffuse_texture())
        .and_then(|t| mats.tex().image(t).map(|i| (i.width as f32, i.height as f32)))
        .map(|(w, h)| (w / h.max(1.0)).round().max(1.0) as u32)
        .unwrap_or(1)
}

/// Builds the sprite mesh of scatter node `node` (node-local space), `None` for nodes that are not scatter objects.
pub(crate) fn build(scene: &Scene, node: usize) -> Option<Arc<Model>> {
    let n = scene.nodes.get(node)?;
    let prim = n.prim_id?;
    let (w, bbox, object_chunk) = scatter_object(&scene.prm, prim)?;
    let material = (w[4] >> 16).saturating_sub(1);
    let count = w[14] as usize;
    let table = scene.prm.chunk_words(w[11] as usize)?;
    let types = scene.prm.chunk(w[13] as usize).unwrap_or_default();
    let count = count.min(table.len().saturating_sub(32) / 4);
    if count == 0 {
        return None;
    }
    let desc = scene.materials.desc(material);
    let f0 = |k: &str, d: f32| desc.as_ref().and_then(|m| m.floats.get(k)).and_then(|v| v.first().copied()).unwrap_or(d);
    let (width, height, rnd_h) = (f0("ScatterWidth", 100.0), f0("ScatterHeight", 100.0), f0("ScatterRandomHeight", 0.0));
    let cols = atlas_columns(&scene.materials, material);
    let size = [bbox.max[0] - bbox.min[0], bbox.max[1] - bbox.min[1], bbox.max[2] - bbox.min[2]];
    let to_world = n.world;
    let to_local = to_world.inverse_affine();

    // ground candidates: level geometry overlapping the volume (world space) with an upward facing triangle in the vertical slab
    let wb = bbox.transformed(&to_world);
    let (y0, y1) = (wb.min[1] - 15.0, wb.max[1] + 15.0);
    let mut tris: Vec<Tri> = Vec::new();
    {
        for (i, other) in scene.nodes.iter().enumerate() {
            if i == node || other.kind != crate::scene::NodeKind::Static || !is_ground_name(&other.name) {
                continue;
            }
            let Some(p) = other.prim_id else { continue };
            let Some(h) = scene.prm.model(p as usize) else { continue };
            let ob = Aabb { min: h.bbox_min, max: h.bbox_max }.transformed(&other.world);
            if ob.max[0] < wb.min[0] || ob.min[0] > wb.max[0] || ob.max[2] < wb.min[2] || ob.min[2] > wb.max[2] || ob.max[1] < y0 || ob.min[1] > y1 {
                continue;
            }
            let Some(m) = scene.model(p) else { continue };
            for mesh in m.meshes.iter().filter(|m| m.is_lod0() && !m.is_skinned()) {
                for t in mesh.indices.chunks_exact(3) {
                    let v: Vec<[f32; 3]> = t.iter().map(|&i| other.world.transform_point(mesh.vertices.positions[i as usize])).collect();
                    let e1 = [v[1][0] - v[0][0], v[1][1] - v[0][1], v[1][2] - v[0][2]];
                    let e2 = [v[2][0] - v[0][0], v[2][1] - v[0][1], v[2][2] - v[0][2]];
                    // front faces are counter-clockwise: the outward normal is -(e1 x e2)
                    let c = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
                    let len = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
                    if len < 1e-3 || -c[1] / len < 0.4 {
                        continue;
                    }
                    let (lo, hi) = (v[0][1].min(v[1][1]).min(v[2][1]), v[0][1].max(v[1][1]).max(v[2][1]));
                    if hi < y0 || lo > y1 {
                        continue;
                    }
                    tris.push(Tri { a: v[0], b: v[1], c: v[2] });
                }
            }
        }
    }

    let mut verts = VertexData::default();
    let mut indices: Vec<u32> = Vec::new();
    let mut placed_on_ground = 0usize;
    for i in 0..count {
        let r = &table[32 + 4 * i..32 + 4 * i + 4];
        let (a, b, c) = (f32::from_bits(r[0]), f32::from_bits(r[1]), f32::from_bits(r[3]));
        let lx = bbox.min[0] + a * size[0];
        let lz = bbox.min[2] + b * size[2];
        // world position of the column, then the ground below it
        let wp = to_world.transform_point([lx, bbox.min[1], lz]);
        let ground = if tris.is_empty() { None } else { ground_at(&tris, wp[0], wp[2]) };
        let (anchor_world_y, on_ground) = match ground {
            Some(y) => (y, true),
            None => (to_world.transform_point([lx, bbox.min[1] + c * size[1], lz])[1], false),
        };
        placed_on_ground += on_ground as usize;
        // slopes of the backdrop mountains: an instance without ground below it would hang in the air
        // (the cliff-side clumps of those volumes render as floating green blocks, so only the grounded ones stay and are trimmed)
        if size[1] >= 2500.0 && (!on_ground || i % 3 != 0) {
            continue;
        }
        let p = to_local.transform_point([wp[0], anchor_world_y, wp[2]]);
        let ty = types.get(i).copied().unwrap_or(0) as u32;
        let frame = ty % cols;
        let (u0, u1) = (frame as f32 / cols as f32, (frame + 1) as f32 / cols as f32);
        let h = height + rnd_h * (c * 2.0 - 1.0);
        let (hw, yaw) = (width * 0.5, hash01(i as u32, w[22]) * std::f32::consts::PI);
        // ground clumps stand on the ground, crown clumps are centred on their anchor
        let (y_lo, y_hi) = if on_ground { (p[1], p[1] + h) } else { (p[1] - h * 0.5, p[1] + h * 0.5) };
        for q in 0..2 {
            let ang = yaw + q as f32 * std::f32::consts::FRAC_PI_2;
            let (s, co) = ang.sin_cos();
            let (dx, dz) = (co * hw, s * hw);
            let base = verts.positions.len() as u32;
            let corners = [([p[0] - dx, y_lo, p[2] - dz], [u0, 1.0]), ([p[0] + dx, y_lo, p[2] + dz], [u1, 1.0]), ([p[0] + dx, y_hi, p[2] + dz], [u1, 0.0]), ([p[0] - dx, y_hi, p[2] - dz], [u0, 0.0])];
            for (pos, uv) in corners {
                verts.positions.push(pos);
                verts.normals.push([0.0, 1.0, 0.0]);
                verts.tangents.push([1.0, 0.0, 0.0, 1.0]);
                verts.uvs.push(uv);
                verts.colors.push([255; 4]);
            }
            // both windings: the sprites are double sided
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3, base, base + 2, base + 1, base, base + 3, base + 2]);
        }
    }
    let _ = placed_on_ground;
    let mut bb = Aabb::from_points(verts.positions.iter());
    if bb.is_empty() {
        bb = bbox;
    }
    let mesh = Mesh {
        layout: VertexLayout::Standard28,
        vertices: verts,
        submeshes: vec![SubMesh { first_index: 0, index_count: indices.len() as u32, material }],
        indices,
        source_chunk: object_chunk,
        variant: w[0],
        group: w[3],
        flags: w[7],
        extra_words: w[16..].to_vec(),
        part: 0,
        bone_palette: Vec::new(),
        palette_words: Vec::new(),
        name: "scatter".into(),
    };
    Some(Arc::new(Model { prim_id: prim, bbox: bb, meshes: vec![mesh], skeleton: None }))
}
