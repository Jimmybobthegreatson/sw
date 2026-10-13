//! Survey of the UI data of one scene: node tree below ROOT with ZWINPIC properties, font-like textures,
//! LOC top level. `cargo run -p bm-ui --example ui_survey -- Scenes/HitmanBloodMoney [filter]`
use bm_formats::loader::SceneFiles;
use bm_vfs::Vfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let scene = args.first().cloned().unwrap_or("Scenes/HitmanBloodMoney".into());
    let filter = args.get(1).cloned().unwrap_or_default().to_lowercase();
    let vfs = Vfs::from_env().unwrap();
    let sf = SceneFiles::load(&vfs, &scene).unwrap();
    let g = sf.graph().unwrap();
    let mut counts = std::collections::BTreeMap::new();
    for n in &g.nodes {
        *counts.entry(format!("{:#x} {:?}", n.type_id, n.type_name())).or_insert(0) += 1;
    }
    println!("types: {counts:#?}");
    for n in &g.nodes {
        if !filter.is_empty() && !g.path(n.index).to_lowercase().contains(&filter) {
            continue;
        }
        println!("{:>5} {}{} [{:#x}] props={:?}", n.index, "  ".repeat(g.depth(n.index)), n.name, n.type_id, n.props);
    }
    if std::env::var("PROBE").is_ok() { probe(&sf, &g); }
    if let Ok(r) = std::env::var("CHUNKS") { let v: Vec<usize> = r.split('-').map(|x| x.parse().unwrap()).collect(); chunks(&sf, v[0], v[1]); }
    if std::env::var("GRP").is_ok() { winpic_groups(&g); }
    if std::env::var("SLOTS").is_ok() { slot_probe(&sf); }
    println!("---- textures");
    for i in sf.tex.images() {
        println!("tex {} {} {}x{} {}", i.id, i.name, i.width, i.height, i.format.name());
    }
}

#[allow(dead_code)]
fn probe(sf: &SceneFiles, g: &bm_formats::scene::SceneGraph) {
    use bm_formats::prp::Value;
    for n in &g.nodes {
        if n.type_id != 0x200046 { continue; }
        let id = match n.props.last() { Some(Value::I32(v)) => *v as usize, _ => continue };
        let c = sf.prm.chunks.get(id);
        let words = sf.prm.chunk_words(id).map(|w| w.into_iter().take(16).collect::<Vec<_>>());
        println!("WINPIC {} id={} chunk={:?} words={:x?}", n.name, id, c, words);
    }
}

#[allow(dead_code)]
fn chunks(sf: &SceneFiles, a: usize, b: usize) {
    for i in a..=b {
        if let Some(w) = sf.prm.chunk_words(i) {
            println!("chunk {i} size {} : {:x?}", w.len() * 4, w);
        }
    }
}

#[allow(dead_code)]
fn winpic_groups(g: &bm_formats::scene::SceneGraph) {
    for n in &g.nodes {
        if n.type_id == 0x200046 || n.type_id == 0x200038 || n.type_id == 0x20004c {
            continue;
        }
        let kids: Vec<String> = n.children.iter().map(|&c| format!("{}[{:x}]", g.nodes[c].name, g.nodes[c].type_id)).collect();
        if n.children.iter().any(|&c| matches!(g.nodes[c].type_id, 0x200046 | 0x200038)) {
            println!("GRP {} -> {}", g.path(n.index), kids.join(", "));
        }
    }
}

#[allow(dead_code)]
pub fn slot_probe(sf: &SceneFiles) {
    for s in [1825u32, 1831, 0x212, 0x213, 0x225, 530, 609] {
        let e = sf.tex.entry_at_slot(s);
        let by_id = sf.tex.image(s).map(|i| (i.name.clone(), i.width, i.height));
        println!("slot {s}: {:?} | id {s}: {:?}", e.map(|e| match e { bm_formats::tex::TexEntry::Image(i) => format!("img {} {} {}x{}", i.id, i.name, i.width, i.height), o => format!("{o:?}").chars().take(60).collect() }), by_id);
    }
}
