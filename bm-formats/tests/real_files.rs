//! Round-trip / sanity tests against the user's own game files. Every test skips gracefully
//! when `BM_GAME_DIR` (or the default install path) is absent.

use bm_formats::{buf::Buf, gms::Gms, prp::Prp, scene::SceneGraph};
use bm_vfs::{game_dir_if_present, Vfs};

fn vfs() -> Option<Vfs> {
    let Some(dir) = game_dir_if_present() else {
        eprintln!("BM_GAME_DIR not available, skipping");
        return None;
    };
    Some(Vfs::new(dir).unwrap())
}

/// Every scene base name (`Scenes/M01/M01_main`) that has a PRP in the game.
fn all_scene_bases(vfs: &Vfs) -> Vec<String> {
    vfs.list("Scenes/**/*.prp")
        .into_iter()
        .map(|e| e.path[..e.path.len() - 4].to_string())
        .collect()
}

#[test]
fn prp_roundtrip_all_scenes() {
    let Some(vfs) = vfs() else { return };
    let bases = all_scene_bases(&vfs);
    assert!(bases.len() > 20);
    for s in &bases {
        let bytes = vfs.open(&format!("{s}.PRP")).unwrap();
        let prp = Prp::parse(&bytes).unwrap_or_else(|e| panic!("{s}: {e}"));
        assert_eq!(prp.to_bytes(), bytes, "{s}: PRP round trip");
        let scene = prp.scene().unwrap_or_else(|e| panic!("{s}: {e}"));
        // stream ends with Bool(false) + EndOfStream
        assert_eq!(scene.trailer_start + 2, prp.instrs.len(), "{s}");
    }
}

#[test]
fn gms_parse_and_scene_join_all_scenes() {
    let Some(vfs) = vfs() else { return };
    for s in all_scene_bases(&vfs) {
        let gms = Gms::parse(&vfs.open(&format!("{s}.GMS")).unwrap()).unwrap_or_else(|e| panic!("{s}: {e}"));
        // re-serialised (uncompressed and compressed) bodies decode back identically
        for compress in [false, true] {
            let re = Gms::parse(&gms.to_bytes(compress).unwrap()).unwrap();
            assert_eq!(re.body, gms.body, "{s} compress={compress}");
        }
        let buf = Buf::parse(&vfs.open(&format!("{s}.BUF")).unwrap()).unwrap();
        let prp = Prp::parse(&vfs.open(&format!("{s}.PRP")).unwrap()).unwrap();
        let graph = SceneGraph::build(&gms, &buf, &prp).unwrap_or_else(|e| panic!("{s}: {e}"));
        assert_eq!(graph.nodes.len(), gms.entries.len() + 1);
        assert!(graph.nodes.iter().skip(1).all(|n| !n.name.is_empty()), "{s}: empty geom name");
        // the stats table lists at least every geometry type that occurs in the table
        for e in 0..gms.entries.len() {
            let t = gms.geom(e).unwrap().type_id;
            assert!(gms.stats.iter().any(|st| st.type_id == t), "{s}: type {t:#x} missing from stats");
        }
        // structural cross-check: groups have children, std objects never do
        for n in &graph.nodes {
            if n.type_id == bm_formats::scene::TYPE_STDOBJ {
                assert!(n.children.is_empty(), "{s}: ZSTDOBJ {} has children", n.name);
            }
        }
    }
}

#[test]
fn m01_scene_details() {
    let Some(vfs) = vfs() else { return };
    let s = "Scenes/M01/M01_main";
    let gms = Gms::parse(&vfs.open(&format!("{s}.GMS")).unwrap()).unwrap();
    let buf = Buf::parse(&vfs.open(&format!("{s}.BUF")).unwrap()).unwrap();
    let prp = Prp::parse(&vfs.open(&format!("{s}.PRP")).unwrap()).unwrap();
    let g = SceneGraph::build(&gms, &buf, &prp).unwrap();
    assert_eq!(gms.entries.len(), 8792);
    assert_eq!(g.nodes[1].name, "M01__Backdrop_01");
    assert!(!g.find_by_name("Landscape_Jungle_12").is_empty());
    let n = &g.nodes[2];
    assert_eq!(n.name, "Landscape_Jungle_12");
    assert_eq!(n.type_id, bm_formats::scene::TYPE_STDOBJ);
    let pos = n.position.unwrap();
    assert!((pos[0] + 7015.91).abs() < 0.1 && (pos[1] - 7668.0).abs() < 0.1 && (pos[2] + 23519.6).abs() < 0.1);
    // the same position is stored in the GMS pre-block of the entry
    let gp = gms.geom_position(1).unwrap();
    assert!((gp[0] - pos[0]).abs() < 1e-3 && (gp[2] - pos[2]).abs() < 1e-3);
    // GMS and PRP positions agree for (nearly) every object
    let mut bad = 0;
    for (i, node) in g.nodes.iter().enumerate().skip(1) {
        if let (Some(p), Some(q)) = (node.position, gms.geom_position(i - 1)) {
            if (0..3).any(|k| (p[k] - q[k]).abs() > 0.01) {
                bad += 1;
            }
        }
    }
    assert!(bad < 20, "{bad} position mismatches");
    assert!(n.stdobj_prim_id().is_some());
    assert!(g.definitions.iter().any(|d| d.name == "CameraFOV"));
}

#[test]
fn tex_roundtrip_all_scenes() {
    use bm_formats::tex::Tex;
    let Some(vfs) = vfs() else { return };
    for s in all_scene_bases(&vfs) {
        let bytes = vfs.open(&format!("{s}.TEX")).unwrap();
        let tex = Tex::parse(&bytes).unwrap_or_else(|e| panic!("{s}: {e}"));
        assert!(!tex.entries.is_empty());
        assert!(tex.to_bytes() == bytes, "{s}: TEX round trip");
    }
}

#[test]
fn tex_decode_all_formats() {
    use bm_formats::tex::{Tex, TexFormat};
    let Some(vfs) = vfs() else { return };
    let mut seen = std::collections::HashSet::new();
    for s in ["Scenes/M01/M01_main", "Scenes/HitmanBloodMoney", "Scenes/M00/M00_news"] {
        let tex = Tex::parse(&vfs.open(&format!("{s}.TEX")).unwrap()).unwrap();
        for img in tex.images() {
            let n = img.mips.len();
            // decode the smallest mip of everything and the largest mip of one image per format
            let small = img.decode_rgba(n - 1).unwrap_or_else(|e| panic!("{s} {} {:?}: {e}", img.name, img.format));
            let (w, h) = img.mip_size(n - 1);
            assert_eq!(small.len(), (w * h * 4) as usize);
            if seen.insert(img.format.name()) {
                let big = img.decode_rgba(0).unwrap();
                assert_eq!(big.len(), img.width as usize * img.height as usize * 4);
            }
            let _ = TexFormat::Dxt1;
        }
    }
    assert!(seen.len() >= 5, "formats seen: {seen:?}");
}

#[test]
fn loc_roundtrip_and_lookup() {
    use bm_formats::loc::Loc;
    let Some(vfs) = vfs() else { return };
    let mut n = 0;
    for s in all_scene_bases(&vfs) {
        if !vfs.exists(&format!("{s}.LOC")) {
            continue; // loader scenes have no localisation file
        }
        n += 1;
        let bytes = vfs.open(&format!("{s}.LOC")).unwrap();
        let loc = Loc::parse(&bytes).unwrap_or_else(|e| panic!("{s}: {e}"));
        assert_eq!(loc.to_bytes(), bytes, "{s}: LOC round trip");
    }
    assert!(n > 20);
    let loc = Loc::parse(&vfs.open("Scenes/M01/M01_main.LOC").unwrap()).unwrap();
    assert_eq!(loc.get("AllLevels/Actions/ActionKill").as_deref(), Some("Action kill"));
    assert_eq!(loc.top.len(), 2);
    let leaves = loc.leaves();
    assert!(leaves.len() > 3000);
    // a voiced line carries the stream words
    let (_, l) = leaves.iter().find(|(p, _)| p.ends_with("M01_AAMS_01MovStarAgent")).unwrap();
    assert_eq!(l.flag, bm_formats::loc::FLAG_VOICED);
    assert_eq!(l.stream_words(), Some((0xbe130, 0xbe130)));
    assert!(l.text().starts_with("You know the deal, kid."));
}

#[test]
fn prm_roundtrip_and_model_chain() {
    use bm_formats::prm::Prm;
    let Some(vfs) = vfs() else { return };
    for s in all_scene_bases(&vfs) {
        let bytes = vfs.open(&format!("{s}.PRM")).unwrap();
        let prm = Prm::parse(&bytes).unwrap_or_else(|e| panic!("{s}: {e}"));
        assert_eq!(prm.to_bytes(), bytes, "{s}: PRM round trip");
    }
    // M01: the prim id of every ZSTDOBJ names a model chunk whose first primitive is a sane triangle list
    let s = "Scenes/M01/M01_main";
    let prm = Prm::parse(&vfs.open(&format!("{s}.PRM")).unwrap()).unwrap();
    let gms = Gms::parse(&vfs.open(&format!("{s}.GMS")).unwrap()).unwrap();
    let buf = Buf::parse(&vfs.open(&format!("{s}.BUF")).unwrap()).unwrap();
    let prp = Prp::parse(&vfs.open(&format!("{s}.PRP")).unwrap()).unwrap();
    let g = SceneGraph::build(&gms, &buf, &prp).unwrap();
    let (mut ok, mut total) = (0, 0);
    for n in &g.nodes {
        if let Some(pid) = n.stdobj_prim_id() {
            total += 1;
            let Some(m) = prm.model(pid as usize) else { continue };
            let objs = prm.model_objects(&m);
            let mut good = !objs.is_empty();
            for o in &objs {
                let Some(p) = &o.prim else { good = false; continue };
                let Some(idx) = prm.prim_indices(p) else { good = false; continue };
                if idx.len() % 3 != 0 || idx.iter().any(|&i| (i as u32) >= p.vertex_count) {
                    good = false;
                }
                if prm.prim_vertices(p).is_none() {
                    good = false;
                }
            }
            if good {
                ok += 1;
            }
        }
    }
    eprintln!("ZSTDOBJ models decoded through the chunk graph: {ok}/{total}");
    assert!(total > 4000);
    assert!(ok * 100 >= total * 90, "{ok}/{total} ZSTDOBJ models decode through the chunk graph");
}

#[test]
fn mat_instances() {
    use bm_formats::mat::Mat;
    let Some(vfs) = vfs() else { return };
    for s in all_scene_bases(&vfs) {
        let mat = Mat::parse(&vfs.open(&format!("{s}.MAT")).unwrap()).unwrap_or_else(|e| panic!("{s}: {e}"));
        assert!(!mat.instances.is_empty());
        // the tiny 0x30-byte "Sprites" instances carry no name; every larger one does
        let big: Vec<_> = mat.instances.iter().filter(|i| i.len > 0x60).collect();
        let named = big.iter().filter(|i| !i.name.is_empty()).count();
        assert!(named * 10 >= big.len() * 9, "{s}: only {named}/{} named", big.len());
    }
    let mat = Mat::parse(&vfs.open("Scenes/M01/M01_main.MAT").unwrap()).unwrap();
    let m287 = mat.instance(287).unwrap();
    assert_eq!(m287.name, "ScatterMaterial");
    assert!(m287.texture_ids().contains(&156));
    assert_eq!(m287.shader(&mat).as_deref(), Some("ScatterBillboard.fx"));
    assert_eq!(mat.class_name(7).as_deref(), Some("ScatterMaterial"));
    assert_eq!(mat.class_name(4).as_deref(), Some("Standard"));
    let skin = mat.instance(300).unwrap().textures(&mat);
    assert_eq!(skin[0].slot, "mapDiffuse");
    assert_eq!(skin[0].texture_id, 180);
    assert_eq!(skin[1].slot, "mapNormal");
    assert_eq!(mat.instance(286).unwrap().name, "Locations/M01/Landscape_M01_Jungle_01_Opac");
}

#[test]
fn small_formats() {
    use bm_formats::{anm::Anm, misc::*, oct::Oct, sgp::Sgp, snd::Snd};
    let Some(vfs) = vfs() else { return };
    for s in all_scene_bases(&vfs) {
        let sgp = vfs.open(&format!("{s}.SGP")).unwrap();
        assert_eq!(Sgp::parse(&sgp).unwrap_or_else(|e| panic!("{s}: {e}")).to_bytes(), sgp, "{s}: SGP round trip");
        let snd = vfs.open(&format!("{s}.SND")).unwrap();
        Snd::parse(&snd).unwrap_or_else(|e| panic!("{s}: {e}"));
        let anm = vfs.open(&format!("{s}.ANM")).unwrap();
        Anm::parse(&anm).unwrap_or_else(|e| panic!("{s}: {e}"));
        Oct::parse(&vfs.open(&format!("{s}.OCT")).unwrap()).unwrap();
        RoomMap::parse(&vfs.open(&format!("{s}.RMC")).unwrap()).unwrap();
        RoomMap::parse(&vfs.open(&format!("{s}.RMI")).unwrap()).unwrap();
        assert!(StubFile { data: vfs.open(&format!("{s}.ZGF")).unwrap() }.is_zgf(), "{s}");
        assert!(StubFile { data: vfs.open(&format!("{s}.SUP")).unwrap() }.is_sup(), "{s}");
        assert!(StubFile { data: vfs.open(&format!("{s}.SGD")).unwrap() }.is_sgd_empty(), "{s}");
    }
    let snd = Snd::parse(&vfs.open("Scenes/M01/M01_main.SND").unwrap()).unwrap();
    assert_eq!(snd.stream_name.to_lowercase(), "pc_eng.str");
    assert!(snd.waves().count() > 100);
    let anm = Anm::parse(&vfs.open("Scenes/hideout/Loader_Sequence.ANM").unwrap()).unwrap();
    let sk = anm.skeleton.unwrap();
    assert_eq!((sk.bones.len(), sk.poses.len()), (60, 55));
    assert_eq!(sk.bones[0], "PELVIS");
    let anm = Anm::parse(&vfs.open("Scenes/M01/M01_main.ANM").unwrap()).unwrap();
    assert!(anm.databases.iter().any(|d| d.name == "M01_Don"));
    let sgp = Sgp::parse(&vfs.open("Scenes/M01/M01_main.SGP").unwrap()).unwrap();
    assert_eq!((sgp.planes.len(), sgp.edges.len(), sgp.paths.len()), (95, 544, 78));
    assert!(sgp.indices.iter().all(|&i| (i as usize) < sgp.planes.len()));
}

#[test]
fn material_chain_prm_mat_tex() {
    use bm_formats::{mat::Mat, prm::Prm, tex::Tex};
    let Some(vfs) = vfs() else { return };
    let s = "Scenes/M01/M01_main";
    let prm = Prm::parse(&vfs.open(&format!("{s}.PRM")).unwrap()).unwrap();
    let mat = Mat::parse(&vfs.open(&format!("{s}.MAT")).unwrap()).unwrap();
    let tex = Tex::parse(&vfs.open(&format!("{s}.TEX")).unwrap()).unwrap();
    let (mut objs, mut with_mat, mut with_tex) = (0, 0, 0);
    let mut names = std::collections::BTreeSet::new();
    for mc in prm.model_chunks() {
        let m = prm.model(mc).unwrap();
        for o in prm.model_objects(&m) {
            objs += 1;
            if let Some(mi) = mat.instance(o.material_instance()) {
                with_mat += 1;
                let ids = mi.texture_ids();
                if !ids.is_empty() && ids.iter().all(|&t| tex.image(t).is_some()) {
                    with_tex += 1;
                    names.insert(tex.image(ids[0]).unwrap().name.clone());
                }
            }
        }
    }
    eprintln!("objects {objs}, with material {with_mat}, with decodable texture refs {with_tex}, {} distinct first textures", names.len());
    assert!(with_mat * 100 >= objs * 95, "material ids resolve: {with_mat}/{objs}");
    assert!(with_tex * 100 >= with_mat * 50, "texture ids resolve: {with_tex}/{with_mat}");
}

#[test]
fn scene_files_loader() {
    use bm_formats::loader::SceneFiles;
    let Some(vfs) = vfs() else { return };
    let f = SceneFiles::load(&vfs, "Scenes/M01/M01_main").unwrap();
    let g = f.graph().unwrap();
    assert_eq!(g.nodes.len(), 8793);
    assert!(f.loc.is_some());
    let l = SceneFiles::load(&vfs, "Scenes/M01/Loader_Sequence").unwrap();
    assert!(l.loc.is_none());
}

#[test]
fn roommaps_all_scenes() {
    use bm_formats::roommap::RoomMap;
    let Some(vfs) = vfs() else { return };
    let mut total = 0;
    let mut enclosed = 0;
    let mut centred = 0;
    for s in all_scene_bases(&vfs) {
        for ext in ["RMC", "RMI"] {
            let bytes = vfs.open(&format!("{s}.{ext}")).unwrap();
            let m = RoomMap::parse(&bytes).unwrap_or_else(|e| panic!("{s}.{ext}: {e}"));
            assert_eq!(m.to_bytes(), bytes, "{s}.{ext}: round trip");
            let step = m.grid().step() * 3.0 + 1.0;
            for i in 0..m.leaves.len() {
                let r = m.leaf_record(i).unwrap();
                assert!(r.half.iter().all(|h| *h >= 0.0 && h.is_finite()), "{s}.{ext} record {i}: extents {:?}", r.half);
                assert!(r.axis_aligned <= 1);
                // the quantised leaf box encloses the record's box (a few % of the leaves are tighter than the nominal box)
                let (qlo, qhi) = m.leaf_world_box(i).unwrap();
                let (lo, hi) = r.world_aabb();
                if (0..3).all(|k| qlo[k] <= r.center[k] + step && qhi[k] >= r.center[k] - step) {
                    centred += 1;
                }
                if (0..3).all(|k| qlo[k] <= lo[k] + step && qhi[k] >= hi[k] - step) {
                    enclosed += 1;
                }
                // rotation rows are unit vectors
                for row in 0..3 {
                    let n: f32 = (0..3).map(|c| r.rotation[row * 3 + c].powi(2)).sum();
                    assert!((n - 1.0).abs() < 1e-3, "{s}.{ext} record {i}: rotation row {row} length^2 {n}");
                }
                // the box contains its own centre
                assert!(r.contains(r.center));
                total += 1;
            }
        }
    }
    assert!(total > 1000, "only {total} room boxes decoded");
    assert!(centred * 100 >= total * 98, "only {centred}/{total} record centres lie in their leaf box");
    assert!(enclosed * 100 >= total * 95, "only {enclosed}/{total} leaf boxes enclose their record box");
}

#[test]
fn oct_blocks_all_scenes() {
    use bm_formats::oct::{BlockKind, Oct};
    let Some(vfs) = vfs() else { return };
    for s in all_scene_bases(&vfs) {
        let bytes = vfs.open(&format!("{s}.OCT")).unwrap();
        let o = Oct::parse(&bytes).unwrap_or_else(|e| panic!("{s}: {e}"));
        assert_eq!(o.to_bytes(), bytes);
        for b in &o.blocks {
            for l in &b.leaves {
                let (lo, hi) = b.grid.world_box(&l.qbox);
                assert!((0..3).all(|k| lo[k].is_finite() && hi[k].is_finite() && lo[k] <= hi[k] + 1e-3), "{s}: bad leaf box {lo:?} {hi:?}");
            }
        }
        if s.ends_with("M01/M01_main") {
            assert_eq!(o.blocks.iter().filter(|b| b.kind == BlockKind::A).count(), 149);
            assert_eq!(o.blocks.iter().filter(|b| b.kind == BlockKind::B).count(), 1166);
            assert_eq!((o.header.scale * 100.0).round(), 8.0);
            assert!(o.tail.len() > 1000);
        }
    }
}

#[test]
fn gameplay_nodes_all_scenes() {
    use bm_formats::gameplay::*;
    use bm_formats::loader::SceneFiles;
    let Some(vfs) = vfs() else { return };
    let mut doors_total = 0;
    for s in all_scene_bases(&vfs) {
        if !s.ends_with("_main") {
            continue;
        }
        let sf = SceneFiles::load(&vfs, &s).unwrap_or_else(|e| panic!("{s}: {e}"));
        let g = sf.graph().unwrap();
        let idx = PathIndex::new(&g);
        // doors: every controller is understood
        let ds = doors(&g);
        doors_total += ds.len();
        for d in &ds {
            assert!(["EASY", "MEDIUM", "HARD", "IMPOSSIBLE", "NORMAL", "VERYEASY"].contains(&d.lock_difficulty.as_str()) || !d.lock_difficulty.is_empty(), "{s}: door lock token");
            assert!(d.speed > 0.0 && d.speed <= 5.0, "{s}: door speed {}", d.speed);
        }
        // waypoint lists resolve
        let pf = path_followers(&g);
        let mut total = 0;
        let mut ok = 0;
        for p in &pf {
            for l in &p.lists {
                total += 1;
                if let Some(i) = idx.resolve(l) {
                    if g.nodes[i].type_id == TYPE_ZLIST {
                        ok += 1;
                    }
                }
            }
        }
        assert!(total == 0 || ok * 100 >= total * 95, "{s}: only {ok}/{total} PathFollower lists resolve to ZLIST nodes");
        // the list items resolve to nodes
        let mut items = 0;
        let mut found = 0;
        for n in g.nodes.iter().filter(|n| n.type_id == TYPE_ZLIST) {
            for it in zlist_items(n) {
                items += 1;
                if idx.resolve(&it).is_some() {
                    found += 1;
                }
            }
        }
        assert!(items == 0 || found * 100 >= items * 90, "{s}: only {found}/{items} ZLIST items resolve");
        for v in volumes(&g) {
            assert!(v.size.iter().all(|x| x.is_finite() && *x >= 0.0), "{s}: box size");
        }
        if let Some(loc) = &sf.loc {
            let level = s.rsplit('/').next().unwrap().trim_end_matches("_main").to_string();
            let obj = mission_objectives(loc, &level);
            assert!(!obj.is_empty() || level == "hideout", "{s}: no mission objectives in the LOC");
        }
    }
    assert!(doors_total > 100, "{doors_total} doors");

    // M01 spot checks
    let sf = SceneFiles::load(&vfs, "Scenes/M01/M01_main").unwrap();
    let g = sf.graph().unwrap();
    let ds = doors(&g);
    assert_eq!(ds.iter().filter(|d| d.kind == DoorKind::Single).count(), 26);
    assert_eq!(ds.iter().filter(|d| d.kind == DoorKind::Multi).count(), 21);
    assert!(ds.iter().any(|d| d.initial == DoorState::Locked && d.lock_difficulty == "EASY"));
    assert!(ds.iter().any(|d| d.sliding));
    let acts = actors(&g);
    assert_eq!(acts.len(), 71);
    let don = acts.iter().find(|a| g.nodes[a.node].parent.map(|p| g.nodes[p].name.ends_with("Special_characters!DonChilean")).unwrap_or(false)).expect("Don");
    assert_eq!(don.script.as_deref(), Some("m01/m01_donchilean"));
    assert_eq!(don.weapon.as_deref(), Some("Gun_SnubNosed_01"));
    assert_eq!(don.anim_db.as_deref(), Some("M01_Don"));
    assert_eq!(don.path.as_ref().map(|p| p.mode), Some(PathMode::Looping));
    let obj = mission_objectives(sf.loc.as_ref().unwrap(), "M01");
    assert_eq!(obj.iter().map(|o| o.kind).collect::<Vec<_>>(), vec![ObjectiveType::Target, ObjectiveType::Target, ObjectiveType::Escape]);
    assert_eq!(obj[0].text, "Kill Don Fernando Delgado");
    let exits = exit_areas(&g);
    assert_eq!(exits.len(), 1);
    assert_eq!(exits[0].size, [200.0, 200.0, 200.0]);
    let zr = zone_rooms(&g);
    assert!(zr.iter().any(|(z, r)| z == "eZone1_PATCH" && r.len() >= 6));
    assert!(pickups(&g).len() >= 50);
    assert!(!climb_boxes(&g).is_empty() && climb_boxes(&g).iter().all(|c| c.corners.len() == 4));
}

#[test]
fn sgp_portal_graph_all_scenes() {
    use bm_formats::sgp::Sgp;
    let Some(vfs) = vfs() else { return };
    let (mut total, mut ordered, mut weights_ok, mut edges_total) = (0, 0, 0, 0);
    for s in all_scene_bases(&vfs) {
        let sgp = Sgp::parse(&vfs.open(&format!("{s}.SGP")).unwrap()).unwrap();
        let portals = sgp.portals();
        assert_eq!(portals.len(), sgp.planes.len(), "{s}: every plane names two path entries");
        for p in &portals {
            total += 1;
            // each connection is stored once, from the lower to the higher path index
            if p.from < p.to {
                ordered += 1;
            }
        }
        // edge weights are distances between the portal centres of one room
        for e in &sgp.edges {
            edges_total += 1;
            let (a, b) = (&sgp.planes[e.a as usize], &sgp.planes[e.b as usize]);
            let d = (0..3).map(|k| (a.center[k] - b.center[k]).powi(2)).sum::<f32>().sqrt();
            if (d - e.weight).abs() <= 1.0 + 0.01 * d {
                weights_ok += 1;
            }
        }
    }
    assert!(total > 500, "{total} portals");
    assert!(ordered * 100 >= total * 99, "only {ordered}/{total} portals run from the lower to the higher path index");
    assert!(weights_ok * 100 >= edges_total * 95, "only {weights_ok}/{edges_total} edge weights are portal distances");
    // M01: the front door of the hacienda leads from the outside area into a ground floor room
    let sgp = Sgp::parse(&vfs.open("Scenes/M01/M01_main.SGP").unwrap()).unwrap();
    let p = &sgp.portals()[0];
    assert!(sgp.paths[p.from].0.ends_with("M01_Outside_01a") && sgp.paths[p.to].0.contains("Hacienda"), "{:?} -> {:?}", sgp.paths[p.from].0, sgp.paths[p.to].0);
}
