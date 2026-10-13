//! `cargo run -p bm-models --example mat_unnamed -- <scene>` - materials whose name could not be read (the name slot holds a
//! record tag such as `EMAN`/`ATSR`): blend state, textures with format and whether the image has alpha.
use bm_models::scene::Scene;
use bm_vfs::Vfs;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M06/M06_main".into());
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    for id in 1..4096u32 {
        let Some(d) = scene.materials.desc(id) else { continue };
        let garbled = d.name.starts_with("EMAN") || d.name.starts_with("ATSR") || d.name.is_empty() || !d.name.chars().all(|c| c.is_ascii_graphic() || c == ' ');
        if !garbled {
            continue;
        }
        let tex: Vec<String> = d
            .textures
            .iter()
            .map(|t| {
                let i = scene.materials.tex().image(t.texture_id);
                let alpha = scene.materials.image(t.texture_id).map(|im| im.has_alpha()).unwrap_or(false);
                format!("{}={} [{}{}]", t.slot, t.texture_id, i.map(|i| i.format.name()).unwrap_or("-".into()), if alpha { " alpha" } else { "" })
            })
            .collect();
        println!("#{id} {:?} blend {:?} opac {:.2} atst {} | {}", d.name.chars().take(8).collect::<String>(), d.render_state.blend, d.render_state.opacity, d.render_state.alpha_test, tex.join(" "));
    }
}
