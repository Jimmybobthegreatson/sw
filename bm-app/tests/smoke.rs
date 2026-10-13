//! End-to-end smoke test: `bm screenshot` of the hideout renders a non-empty PNG. Skipped without a game install / GPU.

use std::process::Command;

#[test]
fn screenshot_hideout() {
    if bm_vfs::game_dir_if_present().is_none() {
        eprintln!("no game install: skipped");
        return;
    }
    let out = std::env::temp_dir().join("bm_smoke_hideout.png");
    let _ = std::fs::remove_file(&out);
    let r = Command::new(env!("CARGO_BIN_EXE_bm"))
        .args(["--no-mods", "screenshot", "--level", "hideout", "--ticks", "20", "--width", "320", "--height", "180", "--out"])
        .arg(&out)
        .output()
        .expect("run bm");
    let stderr = String::from_utf8_lossy(&r.stderr);
    if stderr.contains("no wgpu adapter") {
        eprintln!("no GPU: skipped");
        return;
    }
    assert!(r.status.success(), "bm failed: {stderr}");
    let len = std::fs::metadata(&out).expect("png written").len();
    assert!(len > 2000, "png too small: {len}");
}
