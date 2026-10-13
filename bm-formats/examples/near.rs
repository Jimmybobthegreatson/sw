//! `... --example near -- scene x y z [radius]`: scene nodes near a (parent-local stored) position.
use bm_formats::loader::SceneFiles;
use bm_vfs::Vfs;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let vfs = Vfs::from_env().unwrap();
    let sf = SceneFiles::load(&vfs, &a[0]).unwrap();
    let g = sf.graph().unwrap();
    let p: Vec<f32> = a[1..4].iter().map(|s| s.parse().unwrap()).collect();
    let r: f32 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(2.0);
    for n in &g.nodes {
        if let Some(q) = n.position {
            let d = ((q[0]-p[0]).powi(2)+(q[1]-p[1]).powi(2)+(q[2]-p[2]).powi(2)).sqrt();
            if d < r { println!("[{}] {:#x} {:?} d={:.2} {}", n.index, n.type_id, q, d, g.path(n.index)); }
        }
    }
}
