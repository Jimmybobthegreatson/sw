//! Research tool: hex dump of a `.SND` (or any VFS file) range. Usage: hexat <vfs path> <hex off> <len>
use bm_vfs::Vfs;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vfs = Vfs::from_env().unwrap();
    let d = vfs.open(&a[1]).unwrap();
    let start = usize::from_str_radix(a[2].trim_start_matches("0x"), 16).unwrap();
    let n: usize = a[3].parse().unwrap();
    let end = (start + n).min(d.len());
    for (i, ch) in d[start..end].chunks(16).enumerate() {
        let hex: Vec<String> = ch.iter().map(|b| format!("{b:02x}")).collect();
        let asc: String = ch.iter().map(|&b| if (32..127).contains(&b) { b as char } else { '.' }).collect();
        println!("{:06x}  {}  {}", start + i * 16, hex.join(" "), asc);
    }
}
