//! Research tool: hex-dumps the body of `.SND` objects whose path contains a substring.
//! Usage: snd_dump <scene-base> <substring> [bytes]
use bm_sound::snd_db::SndDb;
use bm_vfs::Vfs;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let base = &a[1];
    let pat = a[2].to_ascii_lowercase();
    let n: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(256);
    let vfs = Vfs::from_env().unwrap();
    let d = vfs.open(&format!("{base}.SND")).unwrap();
    let db = SndDb::parse(&d);
    for o in db.objects.iter().filter(|o| o.path.contains(&pat)) {
        println!("== {:?} {} name_off={:#x} body={:?} count={}", o.kind, o.path, o.name_off, o.body_off, o.count);
        let start = o.body_off.unwrap_or(o.name_off.saturating_sub(0x20)) as usize;
        let end = (start + n).min(d.len());
        for (i, ch) in d[start..end].chunks(16).enumerate() {
            let hex: Vec<String> = ch.iter().map(|b| format!("{b:02x}")).collect();
            let asc: String = ch.iter().map(|&b| if (32..127).contains(&b) { b as char } else { '.' }).collect();
            println!("  {:06x}  {}  {}", start + i * 16, hex.join(" "), asc);
        }
    }
}
