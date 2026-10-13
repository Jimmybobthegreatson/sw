//! Prints a summary (or one window in detail) of the shipped menu XML.
//! `cargo run -p bm-ui --example menu_dump -- [name] [Window]`
use bm_ui::*;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let vfs = std::sync::Arc::new(bm_vfs::Vfs::from_env().expect("game dir"));
    let mut ui = UiSystem::new(vfs);
    let names: Vec<String> = match a.first() {
        Some(n) => vec![n.clone()],
        None => ["default", "frontend", "online", "saveandc", "briefing", "ingame", "inventor", "postmiss", "premiss"].iter().map(|s| s.to_string()).collect(),
    };
    for n in names {
        let Some(doc) = ui.load_menu(&n) else { println!("{n}: missing"); continue };
        let items: usize = doc.windows.iter().map(|w| w.walk().len()).sum();
        println!("{n}: {} windows, {items} items, {} colour sets, {} button graphics, {} warnings", doc.windows.len(), doc.color_sets.len(), doc.button_graphics.len(), doc.warnings.len());
        if let Some(w) = a.get(1).and_then(|w| doc.window(w)) {
            println!("{:#?}", w);
        } else if a.len() == 1 {
            for w in &doc.windows {
                println!("  {:<32} {}x{} items={} pop_on_back={} bg={:?}", w.name, w.w, w.h, w.walk().len(), w.pop_on_back, w.background);
            }
        }
    }
}
