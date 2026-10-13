//! `... --example sgp -- scene`: dumps the in-game map graph.
use bm_formats::loader::SceneFiles;
use bm_vfs::Vfs;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let vfs = Vfs::from_env().unwrap();
    let sf = SceneFiles::load(&vfs, &a[0]).unwrap();
    let s = &sf.sgp;
    println!("planes {} edges {} paths {} packed {} indices {}", s.planes.len(), s.edges.len(), s.paths.len(), s.packed.len(), s.indices.len());
    for (i, (p, k)) in s.paths.iter().enumerate().take(80) { println!("path {i}: {p} #{k}  rec {:08x?}", s.records[i]); }
    for (i, p) in s.planes.iter().enumerate().take(12) { println!("plane {i}: head {:x?} center {:?} corners0 {:?}", p.head, p.center, p.corners[0]); }
    for e in s.edges.iter().take(30) { println!("edge {} -> {} w {}", e.a, e.b, e.weight); }
    println!("packed[..24] {:08x?}", &s.packed[..24.min(s.packed.len())]);
    println!("indices[..40] {:?}", &s.indices[..40.min(s.indices.len())]);
}
