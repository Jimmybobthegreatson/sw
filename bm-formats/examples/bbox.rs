//! `... --example bbox -- scene node...`: model bbox, local pos and stored rotation of nodes.
use bm_formats::loader::SceneFiles;
use bm_vfs::Vfs;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let vfs = Vfs::from_env().unwrap();
    let sf = SceneFiles::load(&vfs, &a[0]).unwrap();
    let g = sf.graph().unwrap();
    for s in &a[1..] {
        let i: usize = s.parse().unwrap();
        let n = &g.nodes[i];
        let prim = n.props.get(4).and_then(|v| v.as_i32()).unwrap_or(0);
        let bb = sf.prm.model(prim as usize).map(|m| (m.bbox_min, m.bbox_max));
        println!("[{i}] {} pos={:?} rot={:?} bbox={:?}", n.name, n.position, n.rotation.map(|r| r.map(|x| (x * 100.0).round() / 100.0)), bb);
    }
}
