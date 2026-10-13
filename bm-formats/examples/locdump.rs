//! `... --example locdump -- scene [substr]`: prints LOC leaves whose path contains substr.
use bm_formats::loader::SceneFiles;
use bm_vfs::Vfs;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let vfs = Vfs::from_env().unwrap();
    let sf = SceneFiles::load(&vfs, &a[0]).unwrap();
    let f = a.get(1).cloned().unwrap_or_default().to_lowercase();
    for (p, t) in sf.loc.as_ref().unwrap().leaves() {
        if p.to_lowercase().contains(&f) { println!("{p} = {}", t.text().chars().take(100).collect::<String>().replace(char::from(10u8), " ")); }
    }
}
