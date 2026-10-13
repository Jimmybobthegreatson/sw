//! Loose-file music modding: replacement audio and custom cue-definition files.
//!
//! Everything goes through the VFS, so a mod directory added with `Vfs::add_mod_dir` simply contains:
//!
//! ```text
//! Music/<level>/<cue>.ogg                 replaces the primary stem of cue <cue> in level <level> (e.g. M01)
//! Music/<level>/<cue>.<stem>.ogg          replaces one named stem of a layered cue
//! Music/<level>/<track>.ogg               replaces a track by its key in the definition (all cues using it)
//! Music/_global/...                       same three forms for every level
//! Music/<level>/*.ron | *.json            extra cue definitions merged over the level's definition
//! Music/_global/*.ron | *.json            extra definitions for all levels
//! ```
//!
//! Extensions tried in order: `ogg`, `flac`, `wav` (decoded with symphonia). Names are case-insensitive.

use bm_vfs::Vfs;

pub const AUDIO_EXTS: [&str; 3] = ["ogg", "flac", "wav"];

/// `"Scenes/M01/M01_main"` -> `"M01"`; anything else maps to its file stem.
pub fn level_id(scene_base: &str) -> String {
    let p = scene_base.replace('\\', "/");
    let parts: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() >= 3 && parts[0].eq_ignore_ascii_case("scenes") {
        parts[1].to_string()
    } else {
        parts.last().copied().unwrap_or("").to_string()
    }
}

/// Lower-case, filesystem friendly key for a bank sound name: `"Music/Licensed/M02_Wonderous.wav"` ->
/// `"m02_wonderous"` (last path segment without extension; spaces become `_`).
pub fn track_key(name: &str) -> String {
    let n = name.replace('\\', "/");
    let last = n.rsplit('/').next().unwrap_or(&n);
    let stem = match last.rfind('.') {
        Some(i) if i > 0 => &last[..i],
        _ => last,
    };
    stem.to_ascii_lowercase().replace(' ', "_")
}

/// Candidate VFS paths (without extension), most specific first.
pub fn override_bases(level: &str, cue: &str, stem: &str, primary: bool, track: &str) -> Vec<String> {
    let mut v = Vec::new();
    for dir in [level, "_global"] {
        v.push(format!("Music/{dir}/{cue}.{stem}"));
        if primary {
            v.push(format!("Music/{dir}/{cue}"));
        }
        v.push(format!("Music/{dir}/{track}"));
    }
    v
}

/// First existing loose audio file for the stem, if a mod provides one.
pub fn find_override(vfs: &Vfs, level: &str, cue: &str, stem: &str, primary: bool, track: &str) -> Option<String> {
    for base in override_bases(level, cue, stem, primary, track) {
        for ext in AUDIO_EXTS {
            let p = format!("{base}.{ext}");
            if vfs.stat(&p).is_some() {
                return Some(p);
            }
        }
    }
    None
}

/// Definition files for a level in merge order (global first, then level; alphabetical inside a group).
pub fn definition_files(vfs: &Vfs, level: &str) -> Vec<String> {
    let mut out = Vec::new();
    for dir in ["_global", level] {
        for ext in ["ron", "json"] {
            let mut v: Vec<String> = vfs.list(&format!("Music/{dir}/*.{ext}")).into_iter().map(|e| e.path).collect();
            v.sort_by_key(|s| s.to_ascii_lowercase());
            out.extend(v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_keys() {
        assert_eq!(level_id("Scenes/M01/M01_main"), "M01");
        assert_eq!(level_id("Scenes\\M06\\M06_main"), "M06");
        assert_eq!(level_id("Scenes/hideout/hideout_main"), "hideout");
        assert_eq!(track_key("Music/Licensed/13 Schubert Ave Maria.wav"), "13_schubert_ave_maria");
        assert_eq!(track_key("Music/Exploration.wav"), "exploration");
    }

    #[test]
    fn candidate_order() {
        let c = override_bases("M01", "explore", "main", true, "exploration");
        assert_eq!(c[0], "Music/M01/explore.main");
        assert_eq!(c[1], "Music/M01/explore");
        assert_eq!(c[2], "Music/M01/exploration");
        assert_eq!(c[3], "Music/_global/explore.main");
        let c = override_bases("M01", "explore", "drums", false, "x");
        assert_eq!(c.len(), 4);
    }
}
