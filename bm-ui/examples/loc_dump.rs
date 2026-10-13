use bm_formats::loader::SceneFiles;
use bm_vfs::Vfs;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let vfs = Vfs::from_env().unwrap();
    let sf = SceneFiles::load(&vfs, &a[0]).unwrap();
    let loc = sf.loc.unwrap();
    let f = a.get(1).cloned().unwrap_or_default().to_lowercase();
    for (p, l) in loc.leaves() {
        if f.is_empty() || p.to_lowercase().contains(&f) { println!("{p} = {:?}", l.text()); }
    }
}
