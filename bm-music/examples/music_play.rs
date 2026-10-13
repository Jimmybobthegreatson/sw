//! Interactive player: plays a level's music on the default audio device and takes commands on stdin.
//! Needs `--features device`.
//!
//! ```text
//! cargo run --release -p bm-music --features device --example music_play -- [scene] [--mod <dir>]
//! commands:  state <name> | event <name> | zone <id> | int <0..1> | stinger <name> | duck <depth>
//!            unduck | pause | resume | vol <0..1> | status | states | quit
//! ```
use bm_music::{load_level_def, MusicOptions};
use bm_sound::device::DeviceOutput;
use bm_sound::SoundLibrary;
use bm_vfs::Vfs;
use std::io::BufRead;
use std::sync::Arc;

fn main() {
    let mut scene = "Scenes/M01/M01_main".to_string();
    let mut vfs = Vfs::from_env().expect("game dir");
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--mod" => vfs.add_mod_dir(it.next().expect("--mod <dir>")).expect("mod dir"),
            s => scene = s.to_string(),
        }
    }
    let vfs = Arc::new(vfs);
    let lib = Arc::new(SoundLibrary::open_level(vfs.clone(), &scene).expect("bank"));
    let set = load_level_def(vfs, lib, &scene).expect("music");
    let (mut engine, _out) = DeviceOutput::start_default().expect("audio device");
    let mut sys = set.system(MusicOptions::for_engine(&engine)).expect("music system");
    sys.attach(&mut engine).expect("attach");
    println!("playing {} ({} states). type `states` or `quit`", set.level.level, set.def.states.len());
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap_or_default();
        let mut p = line.split_whitespace();
        let (cmd, arg) = (p.next().unwrap_or(""), p.next().unwrap_or(""));
        match cmd {
            "state" => println!("{}", sys.set_state(arg)),
            "event" => println!("{}", sys.event(arg)),
            "zone" => println!("{}", sys.set_zone(arg.parse().unwrap_or(-1))),
            "int" => sys.set_intensity(arg.parse().unwrap_or(0.0)),
            "stinger" => println!("{}", sys.trigger_stinger(arg)),
            "duck" => sys.duck(arg.parse().unwrap_or(0.3), 0.1, 0.8),
            "unduck" => sys.unduck(),
            "pause" => sys.pause(0.4),
            "resume" => sys.resume(0.4),
            "vol" => sys.set_volume(arg.parse().unwrap_or(1.0), 0.3),
            "status" => println!("{:#?}", sys.status()),
            "states" => println!("{:?}", sys.def().states.keys().collect::<Vec<_>>()),
            "quit" | "q" => break,
            _ => println!("?"),
        }
        for e in sys.poll_events() {
            println!("  {e:?}");
        }
    }
    sys.detach(&mut engine, 0.5);
}
