//! `cargo run -p bm-models --example anm_clips -- [scene] [name filter]` - lists the decoded clip table of a level's `.ANM`
//! (index, frames, fps, streams present, name).
use bm_models::scene::Scene;
use bm_vfs::Vfs;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let filter = a.get(1).cloned().unwrap_or_default().to_lowercase();
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    let Some(bank) = scene.anim_bank() else {
        println!("no clip table");
        return;
    };
    println!("{} clips, {} bone ids, {} facial poses", bank.clip_count(), bank.bone_names.len(), bank.facial_poses.len());
    for c in &bank.clips {
        if c.name.to_lowercase().contains(&filter) {
            let s: String = c.streams.iter().map(|s| if s.is_some() { 'x' } else { '-' }).collect();
            println!("{:4} {:4}f @{:2} {} {}", c.index, c.frames, c.fps, s, c.name);
            if a.get(2).is_some_and(|d| d == "detail") {
                if let Ok(raw) = bank.raw_clip(c.index) {
                    let used: Vec<usize> = (0..raw.mask.len()).filter(|&i| raw.mask[i]).collect();
                    println!("       stream A floats animated: {} | extras: {:?} | root: {}", used.len(), raw.extras.iter().map(|e| e.bone.as_str()).collect::<Vec<_>>(), raw.root.is_some());
                }
            }
        }
    }
}
