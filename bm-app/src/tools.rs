//! Non-interactive subcommands: `dump`, `export`, `mods`, `sound`, `music`.

use crate::config::{Config, GameArgs, Mounted, mount, resolve_scene};
use bm_models::gltf_export::{ExportOptions, export_scene_glb};
use bm_models::scene::Scene;
use std::path::PathBuf;
use std::sync::Arc;

fn open(args: &GameArgs, cfg: &Config) -> Result<Mounted, String> {
    let m = mount(args, cfg)?;
    for n in &m.notes {
        log::info!("{n}");
    }
    Ok(m)
}

/// `bm dump <scene>`: file listing of a scene archive (and what the mods override).
pub fn dump(args: &GameArgs, cfg: &Config, scene: &str, filter: Option<&str>, extract: Option<(&str, PathBuf)>, tree: bool) -> Result<(), String> {
    let m = open(args, cfg)?;
    let base = resolve_scene(&m.vfs, scene);
    let zip = format!("{base}.ZIP");
    if let Some((name, dir)) = extract {
        let bytes = m.vfs.open_in_archive(&zip, name).or_else(|_| m.vfs.open(name)).map_err(|e| format!("{name}: {e}"))?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let file = dir.join(name.rsplit('/').next().unwrap_or(name));
        std::fs::write(&file, &bytes).map_err(|e| e.to_string())?;
        println!("wrote {} ({} bytes)", file.display(), bytes.len());
        return Ok(());
    }
    if tree {
        let sc = Scene::load(&m.vfs, &base).map_err(|e| format!("{base}: {e}"))?;
        println!("{} nodes", sc.nodes.len());
        fn walk(sc: &Scene, i: usize, depth: usize, max: usize, out: &mut usize) {
            if *out >= max {
                return;
            }
            *out += 1;
            let n = &sc.nodes[i];
            println!("{}{} [{}] {:?}{}", "  ".repeat(depth), n.name, n.type_name.unwrap_or("?"), n.kind, if n.prim_id.is_some() { " (model)" } else { "" });
            for &c in &n.children {
                walk(sc, c, depth + 1, max, out);
            }
        }
        let mut count = 0;
        for n in sc.nodes.iter().filter(|n| n.parent.is_none()) {
            walk(&sc, n.index, 0, 4000, &mut count);
        }
        return Ok(());
    }
    let entries = m.vfs.list_archive(&zip).map_err(|e| format!("{zip}: {e}"))?;
    let f = filter.map(|s| s.to_ascii_lowercase());
    let mut total = 0u64;
    let mut n = 0usize;
    for e in &entries {
        if f.as_ref().is_some_and(|f| !e.path.to_ascii_lowercase().contains(f)) {
            continue;
        }
        let over = match m.vfs.stat(&e.path) {
            Some(i) if !matches!(i.source, bm_vfs::Source::Archive(_)) => "  [overridden by loose file]",
            _ => "",
        };
        println!("{:>10}  {}{}", e.size, e.path, over);
        total += e.size;
        n += 1;
    }
    println!("{n} files, {total} bytes in {zip}");
    for sib in m.vfs.list(&format!("{base}.*")) {
        if !sib.path.to_ascii_lowercase().ends_with(".zip") {
            println!("  sibling: {} ({} bytes)", sib.path, sib.size);
        }
    }
    Ok(())
}

pub struct ExportArgs {
    pub scene: String,
    pub out: PathBuf,
    pub node: Option<String>,
    pub tex: u32,
    pub max_nodes: Option<usize>,
    pub native: bool,
}

/// `bm export <scene> out.glb`
pub fn export(args: &GameArgs, cfg: &Config, a: ExportArgs) -> Result<(), String> {
    let m = open(args, cfg)?;
    let base = resolve_scene(&m.vfs, &a.scene);
    let t = std::time::Instant::now();
    let scene = Scene::load(&m.vfs, &base).map_err(|e| format!("{base}: {e}"))?;
    eprintln!("loaded {} nodes in {:.1?}", scene.nodes.len(), t.elapsed());
    let mut o = ExportOptions::default();
    o.max_geometry_nodes = a.max_nodes;
    o.max_texture_dim = if a.tex == 0 { None } else { Some(a.tex) };
    if a.native {
        o.coords = bm_models::gltf_export::CoordSystem::Native;
    }
    if let Some(n) = &a.node {
        o.root = Some(match n.parse::<usize>() {
            Ok(i) => i,
            Err(_) => *scene.graph.find_by_name(n).first().ok_or_else(|| format!("no node named {n}"))?,
        });
    }
    let r = export_scene_glb(&scene, &o);
    std::fs::write(&a.out, &r.glb).map_err(|e| format!("{}: {e}", a.out.display()))?;
    let s = &r.stats;
    eprintln!(
        "wrote {}: {:.1} MB, {} nodes ({} geometry), {} meshes, {} materials, {} textures, {} skins in {:.1?}",
        a.out.display(),
        s.bytes as f64 / 1e6,
        s.nodes,
        s.geometry_nodes,
        s.meshes,
        s.materials,
        s.textures,
        s.skins,
        t.elapsed()
    );
    Ok(())
}

/// `bm mods ...` (delegates to bm-script).
pub fn mods(args: &GameArgs, cfg: &Config, rest: &[String]) -> Result<(), String> {
    let dirs = crate::config::mod_dirs(args, cfg);
    let gd = crate::config::resolve_game_dir(args, cfg);
    let r: Vec<&str> = rest.iter().map(|s| s.as_str()).collect();
    match bm_script::cli::run_mods_command(&dirs, &r, Some(&gd)) {
        Ok(out) => {
            print!("{out}");
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

/// `bm sound list|info|dump|play`
pub fn sound(args: &GameArgs, cfg: &Config, scene: &str, sub: &str, name: Option<&str>, out: Option<PathBuf>) -> Result<(), String> {
    let m = open(args, cfg)?;
    let base = resolve_scene(&m.vfs, scene);
    let lib = bm_sound::SoundLibrary::open_level(m.vfs.clone(), &base).map_err(|e| format!("sound bank {base}: {e}"))?;
    let find = |n: &str| -> Result<bm_sound::SoundId, String> {
        if let Some(i) = n.strip_prefix('#') {
            return Ok(bm_sound::SoundId { bank: 0, index: i.parse().map_err(|_| "bad index")? });
        }
        lib.find(n).or_else(|| lib.search(n).first().map(|i| i.id)).ok_or_else(|| format!("no sound matching {n}"))
    };
    match sub {
        "list" => {
            let hits: Vec<_> = match name {
                Some(n) => lib.search(n),
                None => lib.iter().collect(),
            };
            for i in &hits {
                println!("#{:<5} {:>6} Hz x{} {:>7.2}s{} {}", i.id.index, i.sample_rate, i.channels, i.duration, if i.streamed { " [str]" } else { "      " }, i.name);
            }
            println!("{} sounds in {} bank(s)", hits.len(), lib.bank_count());
        }
        "info" => {
            let id = find(name.ok_or("sound name required")?)?;
            println!("{:#?}", lib.info(id));
        }
        "dump" => {
            let id = find(name.ok_or("sound name required")?)?;
            let pcm = lib.pcm(id).map_err(|e| e.to_string())?;
            let info = lib.info(id).ok_or("no info")?;
            let path = out.unwrap_or_else(|| PathBuf::from(format!("{}.wav", info.name.rsplit('/').next().unwrap_or("sound"))));
            std::fs::write(&path, bm_sound::pcm::wav_i16(&pcm.samples, pcm.channels, pcm.sample_rate)).map_err(|e| e.to_string())?;
            println!("wrote {} ({:.2}s)", path.display(), pcm.duration_secs());
        }
        "play" => {
            #[cfg(feature = "audio")]
            {
                let id = find(name.ok_or("sound name required")?)?;
                let (engine, _dev) = bm_sound::device::DeviceOutput::start_default().map_err(|e| e.to_string())?;
                let info = lib.info(id).ok_or("no info")?;
                let mut player = bm_sound::SoundPlayer::new(Arc::new(lib), Box::new(engine));
                player.play_id(id, &Default::default()).map_err(|e| e.to_string())?;
                println!("playing {} ({:.1}s)", info.name, info.duration);
                std::thread::sleep(std::time::Duration::from_secs_f32(info.duration.clamp(0.5, 120.0) + 0.5));
            }
            #[cfg(not(feature = "audio"))]
            return Err("built without the `audio` feature".into());
        }
        other => return Err(format!("unknown sound command {other:?} (list|info|dump|play)")),
    }
    Ok(())
}

/// `bm music list|play|render`
pub fn music(args: &GameArgs, cfg: &Config, scene: &str, sub: &str, secs: f32, out: Option<PathBuf>) -> Result<(), String> {
    let m = open(args, cfg)?;
    let base = resolve_scene(&m.vfs, scene);
    let lib = Arc::new(bm_sound::SoundLibrary::open_level(m.vfs.clone(), &base).map_err(|e| format!("sound bank {base}: {e}"))?);
    let set = bm_music::load_level_def(m.vfs.clone(), lib, &base).map_err(|e| format!("music {base}: {e}"))?;
    match sub {
        "list" => {
            let l = &set.level;
            println!("== {} music table: {}", l.level, l.def_name.as_deref().unwrap_or("<none>"));
            for s in l.slots.iter().flatten() {
                println!(
                    "slot {:>3} [{}] {:>6.1}s {}",
                    s.slot,
                    bm_music::slot_role(s.slot),
                    s.duration.unwrap_or(0.0),
                    s.file.as_deref().unwrap_or(&s.sound)
                );
            }
            println!("states: {:?}", set.def.states.keys().collect::<Vec<_>>());
            for w in &set.warnings {
                println!("warning: {w}");
            }
        }
        "ron" => println!("{}", set.def.to_ron()),
        "render" => {
            let mut engine = bm_sound::MixerEngine::headless(48000);
            let mut sys = set.system(bm_music::MusicOptions::for_engine(&engine)).map_err(|e| e.to_string())?;
            sys.attach(&mut engine).map_err(|e| e.to_string())?;
            let samples = engine.render_offline(secs, 1024, |_, _| {});
            let path = out.unwrap_or_else(|| PathBuf::from("music.wav"));
            bm_sound::MixerEngine::write_wav(&path, &samples, 48000).map_err(|e| e.to_string())?;
            println!("wrote {} ({secs}s)", path.display());
        }
        "play" => {
            #[cfg(feature = "audio")]
            {
                let (mut engine, _dev) = bm_sound::device::DeviceOutput::start_default().map_err(|e| e.to_string())?;
                let mut sys = set.system(bm_music::MusicOptions::for_engine(&engine)).map_err(|e| e.to_string())?;
                sys.attach(&mut engine).map_err(|e| e.to_string())?;
                println!("playing {} for {secs:.0}s", set.level.level);
                std::thread::sleep(std::time::Duration::from_secs_f32(secs));
                sys.detach(&mut engine, 0.5);
            }
            #[cfg(not(feature = "audio"))]
            return Err("built without the `audio` feature".into());
        }
        other => return Err(format!("unknown music command {other:?} (list|ron|render|play)")),
    }
    Ok(())
}
