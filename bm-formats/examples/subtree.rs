//! `... --example subtree -- scene node [maxn]`: prints positions of the nodes below `node`.
use bm_formats::loader::SceneFiles;
use bm_vfs::Vfs;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let vfs = Vfs::from_env().unwrap();
    let sf = SceneFiles::load(&vfs, &a[0]).unwrap();
    let g = sf.graph().unwrap();
    let root: usize = a[1].parse().unwrap();
    let maxn: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(60);
    let mut stack = vec![root];
    let mut n = 0;
    let mut mn = [f32::MAX; 3];
    let mut mx = [f32::MIN; 3];
    while let Some(i) = stack.pop() {
        let node = &g.nodes[i];
        if let Some(p) = node.position {
            if node.type_id == 0x200002 { for k in 0..3 { mn[k] = mn[k].min(p[k]); mx[k] = mx[k].max(p[k]); } }
            if n < maxn { println!("[{}] {:#x} {:?} {}", i, node.type_id, p, g.path(i).rsplit('/').next().unwrap()); }
            n += 1;
        }
        for c in node.children.iter().rev() { stack.push(*c); }
    }
    println!("nodes {n}; stdobj local-pos bbox {:?} .. {:?}", mn, mx);
}
