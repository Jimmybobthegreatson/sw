//! `... --example bounds -- scene`: ZBOUND nodes with their model bbox and parent room.
use bm_formats::loader::SceneFiles;
use bm_vfs::Vfs;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let vfs = Vfs::from_env().unwrap();
    let sf = SceneFiles::load(&vfs, &a[0]).unwrap();
    let g = sf.graph().unwrap();
    for node in g.nodes.iter().filter(|n| n.type_id == 0x20001c) {
        let prim = node.props.get(4).and_then(|v| v.as_i32()).unwrap_or(0);
        let bb = sf.prm.model(prim as usize).map(|m| (m.bbox_min, m.bbox_max));
        let parent = node.parent.map(|p| g.nodes[p].name.clone()).unwrap_or_default();
        println!("[{}] {} parent={} pos={:?} rot={:?} prim={} bbox={:?}", node.index, node.name, parent, node.position, node.rotation, prim, bb);
    }
}
