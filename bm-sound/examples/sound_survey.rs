//! Research tool: survey of sound data over *all* scenes (SND object kinds, wave joins, room effects,
//! ZSNDOBJ reference resolution, audio controllers). Usage: sound_survey
use bm_formats::{buf::Buf, gms::Gms, prp::Prp, scene::SceneGraph};
use bm_sound::snd_db::*;
use bm_vfs::Vfs;
use std::collections::BTreeMap;

fn main() {
    let vfs = Vfs::from_env().unwrap();
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let (mut waves, mut joined, mut rooms_total, mut rooms_ok) = (0, 0, 0, 0);
    let (mut zs, mut zs_ok, mut zs_zero) = (0, 0, 0);
    let mut ctl: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut scenes = 0;
    for z in vfs.list("Scenes/**/*.ZIP") {
        let base = z.path.trim_end_matches(".ZIP").trim_end_matches(".zip").to_string();
        let Ok(d) = vfs.open(&format!("{base}.SND")) else { continue };
        scenes += 1;
        let db = SndDb::parse(&d);
        for o in &db.objects {
            *kinds.entry(format!("{:?}", o.kind)).or_default() += 1;
            if let SndKind::Other(k) = &o.kind {
                if k == "roomeffect" {
                    rooms_total += 1;
                }
            }
        }
        rooms_ok += db.room_effects.len();
        let bank = bm_vfs::SoundBank::open(&vfs, &base).ok();
        for w in &db.waves {
            waves += 1;
            if let Some(b) = &bank {
                if b.find(&w.file).is_some() {
                    joined += 1;
                }
            }
        }
        let (Ok(g), Ok(b), Ok(p)) = (vfs.open(&format!("{base}.GMS")), vfs.open(&format!("{base}.BUF")), vfs.open(&format!("{base}.PRP"))) else { continue };
        let (Ok(g), Ok(b), Ok(p)) = (Gms::parse(&g), Buf::parse(&b), Prp::parse(&p)) else { continue };
        let Ok(gr) = SceneGraph::build(&g, &b, &p) else { continue };
        for n in &gr.nodes {
            if n.type_id == 0x200012 {
                zs += 1;
                match n.props.get(6).and_then(|v| v.as_i32()) {
                    Some(0) => zs_zero += 1,
                    Some(o) if db.object_at(o as u32).is_some() => zs_ok += 1,
                    _ => {}
                }
            }
            for c in &n.controllers {
                let l = c.name.to_lowercase();
                if l.contains("audio") || l.contains("sound") || l.contains("music") || l.contains("reverb") {
                    let e = ctl.entry(c.name.clone()).or_default();
                    e.0 += 1;
                    // count I32 properties that resolve to SND objects
                    e.1 += c.props.iter().filter(|v| v.as_i32().map_or(false, |o| o > 0 && db.object_at(o as u32).is_some())).count();
                }
            }
        }
    }
    println!("scenes with .SND: {scenes}");
    println!("SND objects by kind: {kinds:?}");
    println!("wave records {waves}, joined to bank by name {joined}");
    println!("roomeffect objects {rooms_total}, with EAX block {rooms_ok}");
    println!("ZSNDOBJ nodes {zs}: resolved {zs_ok}, offset 0 {zs_zero}, other {}", zs - zs_ok - zs_zero);
    for (k, (n, r)) in ctl {
        println!("controller {k:<28} n={n:<6} resolved-I32-props={r}");
    }
}
