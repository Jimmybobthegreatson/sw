//! `cargo run -p bm-script --example mods -- list|order|validate|conflicts|enable|disable|stubs <file>|api-md`
//!
//! Mod roots: `$BM_MODS_DIRS` (path-separated) if set, else `<exe>/Mods` + `<user dir>/Mods`.
use bm_script::package::ModDirs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let a: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    if a.first() == Some(&"api-md") {
        print!("{}", bm_script::spec::api_markdown());
        return;
    }
    let mut dirs = ModDirs::default_dirs();
    if let Some(v) = std::env::var_os("BM_MODS_DIRS") {
        dirs.roots = std::env::split_paths(&v).collect();
    }
    let game = bm_vfs::game_dir_if_present();
    match bm_script::cli::run_mods_command(&dirs, &a, game.as_deref()) {
        Ok(s) => print!("{s}"),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
