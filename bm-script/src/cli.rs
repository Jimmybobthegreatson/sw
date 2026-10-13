//! Implementation of the `bm mods` subcommands (so `bm-app` only has to forward its arguments).
//!
//! ```text
//! bm mods list                 installed mods, state and load order
//! bm mods order                resolved load order + problems
//! bm mods validate [path]      validate one mod folder/zip, or every installed mod
//! bm mods conflicts            which mods override which VFS paths
//! bm mods enable <id>          remove from the disabled list
//! bm mods disable <id>
//! bm mods stubs <file>         write LuaLS API stubs (.d.lua)
//! ```

use crate::manifest::Severity;
use crate::package::*;
use std::fmt::Write;
use std::path::Path;

pub fn run_mods_command(dirs: &ModDirs, args: &[&str], game_dir: Option<&Path>) -> Result<String, ModError> {
    let mut out = String::new();
    let sub = args.first().copied().unwrap_or("list");
    match sub {
        "list" => {
            let m = ModManager::new(dirs.clone())?;
            out.push_str(&m.list_text());
        }
        "order" => {
            let m = ModManager::new(dirs.clone())?;
            let set = m.resolve();
            for (i, r) in set.order.iter().enumerate() {
                writeln!(out, "{:>3}. {} {} (priority {})", i + 1, r.id(), r.manifest.version, r.priority).ok();
            }
            write_problems(&mut out, &set.problems);
        }
        "validate" => {
            if let Some(p) = args.get(1) {
                let d = load_mod(Path::new(p), &dirs.cache_dir)?;
                writeln!(out, "{} {} ({})", d.manifest.id, d.manifest.version, d.root.display()).ok();
                for x in &d.diagnostics {
                    writeln!(out, "  {:?}: {}", x.severity, x.message).ok();
                }
                if d.diagnostics.is_empty() {
                    out.push_str("  ok\n");
                }
                if d.has_errors() {
                    return Err(ModError::Other(format!("{out}validation failed")));
                }
            } else {
                let m = ModManager::new(dirs.clone())?;
                let mut bad = false;
                for d in m.mods() {
                    writeln!(out, "{} {}", d.manifest.id, d.manifest.version).ok();
                    for x in &d.diagnostics {
                        bad |= x.severity == Severity::Error;
                        writeln!(out, "  {:?}: {}", x.severity, x.message).ok();
                    }
                }
                write_problems(&mut out, &m.resolve().problems);
                if bad {
                    return Err(ModError::Other(format!("{out}validation failed")));
                }
            }
        }
        "conflicts" => {
            let m = ModManager::new(dirs.clone())?;
            let set = m.resolve();
            let vfs = game_dir.and_then(|g| bm_vfs::Vfs::new(g).ok());
            let rep = asset_conflicts(&set, vfs.as_ref());
            if rep.conflicts.is_empty() {
                out.push_str("no asset conflicts between mods\n");
            }
            for c in &rep.conflicts {
                writeln!(
                    out,
                    "{}: {} -> winner {}{}",
                    c.path,
                    c.mods.join(" < "),
                    c.winner,
                    if c.overrides_game == Some(true) { " (overrides game file)" } else { "" }
                )
                .ok();
            }
            for (m, files) in &rep.game_overrides {
                writeln!(out, "{m} overrides {} game file(s)", files.len()).ok();
            }
        }
        "enable" | "disable" => {
            let id = args.get(1).ok_or_else(|| ModError::Other(format!("usage: mods {sub} <id>")))?;
            let mut m = ModManager::new(dirs.clone())?;
            m.set_enabled(id, sub == "enable")?;
            writeln!(out, "{id} {}d", sub).ok();
        }
        "stubs" => {
            let p = args.get(1).ok_or_else(|| ModError::Other("usage: mods stubs <file>".into()))?;
            std::fs::write(p, crate::spec::lua_stubs()).map_err(|e| ModError::Io(p.to_string(), e))?;
            writeln!(out, "wrote {p}").ok();
        }
        other => return Err(ModError::Other(format!("unknown mods command {other:?}"))),
    }
    Ok(out)
}

fn write_problems(out: &mut String, problems: &[Problem]) {
    for p in problems {
        writeln!(out, "{:?}: {}", p.severity, p.message).ok();
    }
}
