//! Lists a level's music: the 64-slot track table, zones, and the generated cue/state definition.
//!
//! ```text
//! cargo run --release -p bm-music --example music_list -- [scene] [--ron|--json] [--mod <dir>]
//! ```
//! `scene` defaults to `Scenes/M01/M01_main`. `--ron` / `--json` print the merged definition (the
//! generated defaults plus any mod files) instead of the summary; it is a good starting template for
//! a custom `Music/<level>/*.ron` file.
use bm_music::{slot_role, load_level_def};
use bm_sound::SoundLibrary;
use bm_vfs::Vfs;
use std::sync::Arc;

fn main() {
    let mut scene = "Scenes/M01/M01_main".to_string();
    let mut mode = "summary";
    let mut vfs = Vfs::from_env().expect("BM_GAME_DIR / default game dir");
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--ron" => mode = "ron",
            "--json" => mode = "json",
            "--mod" => vfs.add_mod_dir(it.next().expect("--mod <dir>")).expect("mod dir"),
            s => scene = s.to_string(),
        }
    }
    let vfs = Arc::new(vfs);
    let lib = Arc::new(SoundLibrary::open_level(vfs.clone(), &scene).expect("sound bank"));
    let set = load_level_def(vfs, lib, &scene).expect("level music");
    match mode {
        "ron" => return println!("{}", set.def.to_ron()),
        "json" => return println!("{}", set.def.to_json()),
        _ => {}
    }
    let l = &set.level;
    println!("== {} ({}) music table: {}", l.level, scene, l.def_name.as_deref().unwrap_or("<none>"));
    println!("slot  pct mode  dur(s)  rate ch  flags  role / file");
    for s in l.slots.iter().flatten() {
        println!(
            "{:>3}  {:>4} {:>3} {:>7} {:>6} {} {:>#7x}  [{}] {}",
            s.slot,
            s.volume_pct,
            s.mode,
            s.duration.map(|d| format!("{d:.1}")).unwrap_or("-".into()),
            s.sample_rate.unwrap_or(0),
            s.channels.unwrap_or(0),
            s.flags.unwrap_or(0),
            slot_role(s.slot),
            s.file.as_deref().unwrap_or(&s.sound)
        );
    }
    println!("\nzones ({}):", l.zones.len());
    for z in &l.zones {
        println!("  {:>3} {:<44} {:?} rooms={} -> {}", z.id, z.name, z.kind, z.rooms.len(), set.def.zones.get(&z.id).map(|s| s.as_str()).unwrap_or("-"));
    }
    println!("\nstates:");
    for (n, s) in &set.def.states {
        let mut flags = Vec::new();
        if s.follow_zone { flags.push("follow_zone"); }
        if s.zone_locked { flags.push("zone_locked"); }
        if s.shuffle { flags.push("shuffle"); }
        println!("  {:<12} cue={:<28} playlist={:?} {}", n, s.cue.as_deref().unwrap_or("-"), s.playlist, flags.join(","));
    }
    println!("\nstingers: {:?}", set.def.stingers.keys().collect::<Vec<_>>());
    println!("events:   {:?}", set.def.events.keys().collect::<Vec<_>>());
    if !set.merged_files.is_empty() {
        println!("merged mod files: {:?}", set.merged_files);
    }
    for w in &set.warnings {
        println!("WARNING {w}");
    }
}
