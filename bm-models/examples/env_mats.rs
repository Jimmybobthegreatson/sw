//! `cargo run -p bm-models --example env_mats -- <scene>` - materials with reflection / environment slots, and what their texture ids resolve to.
use bm_formats::tex::TexEntry;
use bm_models::scene::Scene;
use bm_vfs::Vfs;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M03/M03_main".into());
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    for id in 1..4096u32 {
        let Some(d) = scene.materials.desc(id) else { continue };
        let refl = d.textures.iter().any(|t| t.slot == "mapEnvironment" || t.slot == "mapReflectionMask" || t.slot == "mapReflectionFallOff") || d.flag("ReflectionEnabled");
        if !refl {
            continue;
        }
        let slots: Vec<String> = d
            .textures
            .iter()
            .map(|t| {
                let what = match scene.materials.tex().entry_at_slot(t.texture_id) {
                    Some(TexEntry::Cube { faces }) => format!("list{:?}", faces),
                    Some(TexEntry::Image(i)) => format!("img {}x{} {}", i.width, i.height, i.format.name()),
                    None => "-".into(),
                };
                format!("{}={} en={} [{}]", t.slot, t.texture_id, t.enabled, what)
            })
            .collect();
        println!("#{id} '{}' blend {:?} refl {:?} colors {:?} floats {:?}\n     {}", d.name, d.render_state.blend, d.flag("ReflectionEnabled"), d.colors.iter().filter(|(k, _)| k.contains("Refl")).collect::<Vec<_>>(), d.floats.iter().filter(|(k, _)| k.contains("Refl")).collect::<Vec<_>>(), slots.join("\n     "));
    }
}
