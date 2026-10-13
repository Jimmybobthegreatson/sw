//! `... --example gp -- scene`: summary of the decoded gameplay nodes.
use bm_formats::{gameplay::*, loader::SceneFiles};
use bm_vfs::Vfs;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let vfs = Vfs::from_env().unwrap();
    let sf = SceneFiles::load(&vfs, &a[0]).unwrap();
    let g = sf.graph().unwrap();
    let pk = pickups(&g);
    println!("doors {} actors {} pickups {} (types {:?}) volumes {} climb {} usepoints {}", doors(&g).len(), actors(&g).len(), pk.len(), pk.iter().map(|p| p.node_type).collect::<std::collections::BTreeSet<_>>(), volumes(&g).len(), climb_boxes(&g).len(), use_points(&g).len());
    for p in pk.iter().take(80) { println!("  pickup [{}] {:#x} {} x{} range {}", p.node, p.node_type, p.item_id(), p.amount, p.range); }
    for v in volumes(&g) { if matches!(v.kind, VolumeKind::Action{..}) { println!("  action vol [{}] {:?}", v.node, v.kind); } }
}
