//! Research tool: lists music-flagged bank entries and music-related scene nodes/SND objects.
//! Usage: music_survey [banks|nodes|snd] [scene-base]
use bm_formats::{buf::Buf, gms::Gms, prp::{Prp, Value}, scene::SceneGraph};
use bm_sound::snd_db::SndDb;
use bm_vfs::Vfs;
use std::sync::Arc;

fn short(v: &Value, db: &SndDb) -> String {
    match v {
        Value::Str(s) => format!("S\"{s}\""),
        Value::I32(i) => match db.object_at(*i as u32) { Some(o) => format!("I{i}=>{:?}:{}", o.kind, o.path), None => format!("I{i}") },
        Value::Container(c) | Value::Array(c) => format!("[{}]", c.iter().take(8).map(|x| short(x, db)).collect::<Vec<_>>().join(", ")) + &if c.len() > 8 { format!("..{}", c.len()) } else { String::new() },
        o => format!("{o:?}"),
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mode = a.get(1).cloned().unwrap_or("banks".into());
    let vfs = Arc::new(Vfs::from_env().unwrap());
    let bases: Vec<String> = if let Some(b) = a.get(2) { vec![b.clone()] } else {
        let mut v: Vec<String> = vfs.list("Scenes/**/*.WHD").into_iter().map(|e| e.path.trim_end_matches(".WHD").trim_end_matches(".whd").to_string()).collect();
        v.sort(); v
    };
    for base in bases {
        match mode.as_str() {
            "banks" => {
                let lib = match bm_sound::SoundLibrary::open_level(vfs.clone(), &base) { Ok(l) => l, Err(e) => { println!("{base}: {e}"); continue } };
                let mut n = 0;
                for i in lib.iter() {
                    let low = i.name.to_ascii_lowercase();
                    if low.contains("music") || matches!(i.flags, 0x180 | 0x8280) || i.flags & 0x8000 != 0 {
                        if n == 0 { println!("== {base}"); }
                        n += 1;
                        println!("  {:#06x} {}ch {}Hz {:.1}s str={} {}", i.flags, i.channels, i.sample_rate, i.duration, i.streamed, i.name);
                    }
                }
            }
            "defs" => {
                let Ok(d) = vfs.open(&format!("{base}.SND")) else { continue };
                let db = SndDb::parse(&d);
                for o in db.objects.iter().filter(|o| o.path.starts_with("_sounddefinitionsbloodmoney/music/")) {
                    let Some(b) = o.body_off else { continue };
                    let b = b as usize;
                    let u = |p: usize| u32::from_le_bytes(d[p..p + 4].try_into().unwrap());
                    let tp = u(b + 0x14) as usize;
                    println!("== {base} {} body={b:#x} table={tp:#x} slots={}", o.path, (b - tp) / 4);
                    for i in 0..(b - tp) / 4 {
                        let v = u(tp + i * 4);
                        if v == 0 { continue; }
                        match db.object_at(v) {
                            Some(so) => { let w: Vec<String> = db.waves_of(so).iter().map(|w| w.file.clone()).collect(); println!("   [{i:2}] {:?} {} -> {:?} {:?}", so.kind, so.path, w, db.sound_params(so).map(|p| (p.volume_pct, p.mode, p.aux, p.range_m, p.gain_a, p.gain_b))) }
                            None => println!("   [{i:2}] {v:#x} (unresolved)"),
                        }
                    }
                }
            }
            "snd" => {
                let Ok(d) = vfs.open(&format!("{base}.SND")) else { continue };
                let db = SndDb::parse(&d);
                let mut n = 0;
                for o in &db.objects {
                    if o.path.contains("music") { if n == 0 { println!("== {base}"); } n += 1; println!("  {:?} {} (count {})", o.kind, o.path, o.count); }
                }
            }
            _ => {
                let (Ok(gd), Ok(bd), Ok(pd), Ok(sd)) = (vfs.open(&format!("{base}.GMS")), vfs.open(&format!("{base}.BUF")), vfs.open(&format!("{base}.PRP")), vfs.open(&format!("{base}.SND"))) else { continue };
                let db = SndDb::parse(&sd);
                let g = Gms::parse(&gd).unwrap(); let b = Buf::parse(&bd).unwrap(); let p = Prp::parse(&pd).unwrap();
                let gr = SceneGraph::build(&g, &b, &p).unwrap();
                println!("== {base}");
                for nd in &gr.nodes {
                    let lname = nd.name.to_ascii_lowercase();
                    let mut hit = lname.contains("music");
                    for c in &nd.controllers { if c.name.to_ascii_lowercase().contains("music") { hit = true; } }
                    if !hit { continue; }
                    println!(" node {:#x} {}", nd.type_id, nd.name);
                    for c in &nd.controllers {
                        println!("   ctl {}", c.name);
                        for (i, v) in c.props.iter().enumerate() { println!("      [{i}] {}", short(v, &db)); }
                    }
                }
            }
        }
    }
}
