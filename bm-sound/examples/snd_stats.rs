//! Research tool: histogram of the `#sound` body bytes 0x24..0x2b over all scenes (see docs/sound.md).
use bm_sound::snd_db::*;
use std::collections::BTreeMap;
fn main() {
    let vfs = bm_vfs::Vfs::from_env().unwrap();
    let mut hist: BTreeMap<String, (usize, Vec<String>)> = BTreeMap::new();
    let mut total = 0;
    for z in vfs.list("Scenes/**/*.ZIP") {
        let base = z.path.trim_end_matches(".ZIP").trim_end_matches(".zip").to_string();
        let Ok(d) = vfs.open(&format!("{base}.SND")) else { continue };
        let db = SndDb::parse(&d);
        let d = db.raw();
        for o in db.objects.iter().filter(|o| o.kind == SndKind::Sound) {
            let Some(b) = o.body_off else { continue };
            let b = b as usize;
            total += 1;
            let key = format!("b24..2b={:02x?} cnt={}", &d[b + 0x24..b + 0x2c], o.count.min(3));
            let e = hist.entry(key).or_default();
            e.0 += 1;
            if e.1.len() < 3 { e.1.push(o.path.clone()); }
        }
    }
    println!("total {total}");
    let mut v: Vec<_> = hist.into_iter().collect();
    v.sort_by_key(|(_, (n, _))| std::cmp::Reverse(*n));
    for (k, (n, ex)) in v.iter().take(40) { println!("{n:6} {k}  {}", ex.join(" | ")); }
}
