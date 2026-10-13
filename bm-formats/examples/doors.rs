//! `... --example doors -- scene`: nodes with a SingleDoor / MultiDoor controller, their model bbox and controller props.
use bm_formats::loader::SceneFiles;
use bm_vfs::Vfs;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let vfs = Vfs::from_env().unwrap();
    let sf = SceneFiles::load(&vfs, &a[0]).unwrap();
    let g = sf.graph().unwrap();
    for node in &g.nodes {
        for c in &node.controllers {
            if c.name == "SingleDoor" || c.name == "MultiDoor" {
                let prim = node.props.get(4).and_then(|v| v.as_i32()).unwrap_or(0);
                let bb = sf.prm.model(prim as usize).map(|m| (m.bbox_min, m.bbox_max));
                let r = node.rotation.unwrap_or([0.0; 9]);
                println!("[{}] {} {} pos={:?} rot={:?} bbox={:?}", node.index, c.name, g.path(node.index).rsplit('/').next().unwrap(), node.position, [r[0], r[2], r[6], r[8]], bb);
                println!("      {:?}", c.props.iter().map(|v| format!("{v:?}")).collect::<Vec<_>>().join(" "));
            }
        }
    }
}
