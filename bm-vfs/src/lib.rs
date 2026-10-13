//! `bm-vfs` - virtual file system for the Blood Money runtime.
//!
//! A [`Vfs`] merges several sources behind one case-insensitive, `/`-separated namespace:
//!
//! 1. **mod directories** (highest priority, the last one added wins),
//! 2. **loose files** in the game directory,
//! 3. the contents of every **scene ZIP** found under `Scenes/**` (also inside mod dirs).
//!
//! Besides the plain file API this crate understands the two non-ZIP containers the engine ships
//! next to the scenes: sound banks (`.WHD` + `.WAV`, see [`sound`]) and the big voice/music
//! stream `pc_eng.str` (see [`stream`]).
//!
//! No game data is ever copied: everything is read lazily from the user's own installation.

mod glob;
pub mod sound;
pub mod stream;

use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub use sound::{SoundBank, SoundEntry};
pub use stream::{StrEntry, StrIndex};

/// Environment variable naming the Blood Money install directory.
pub const GAME_DIR_ENV: &str = "BM_GAME_DIR";
/// Default install location used when [`GAME_DIR_ENV`] is not set.
pub const DEFAULT_GAME_DIR: &str = r"C:\Users\Shadow\Downloads\Hitman Blood Money";

/// Directory names never descended into while indexing the game directory
/// (the Rust workspace lives inside the game folder on the dev machine).
const SKIP_DIRS: &[&str] = &["bloodmoney-rs", "target", ".git", "node_modules"];

// ---------------------------------------------------------------------------------------------
// errors
// ---------------------------------------------------------------------------------------------

#[derive(Debug)]
pub enum VfsError {
    Io(std::io::Error),
    Zip(String),
    NotFound(String),
    Format(String),
}

pub type Result<T> = std::result::Result<T, VfsError>;

impl fmt::Display for VfsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VfsError::Io(e) => write!(f, "io error: {e}"),
            VfsError::Zip(e) => write!(f, "zip error: {e}"),
            VfsError::NotFound(p) => write!(f, "not found: {p}"),
            VfsError::Format(m) => write!(f, "format error: {m}"),
        }
    }
}

impl std::error::Error for VfsError {}

impl From<std::io::Error> for VfsError {
    fn from(e: std::io::Error) -> Self {
        VfsError::Io(e)
    }
}

impl From<zip::result::ZipError> for VfsError {
    fn from(e: zip::result::ZipError) -> Self {
        VfsError::Zip(e.to_string())
    }
}

// ---------------------------------------------------------------------------------------------
// path helpers
// ---------------------------------------------------------------------------------------------

/// Canonical lookup key: lowercase, `/` separators, no leading `./` or `/`, no empty segments.
pub fn normalize(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for seg in path.split(['/', '\\']) {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(&seg.to_lowercase());
    }
    out
}

// ---------------------------------------------------------------------------------------------
// public data types
// ---------------------------------------------------------------------------------------------

/// Where a VFS entry physically lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// Loose file in the n-th mod directory (index into [`Vfs::mod_dirs`]).
    Mod(usize),
    /// Loose file in the game directory.
    GameDir,
    /// Entry of a ZIP archive; the string is the archive's VFS path (e.g. `Scenes/M01/M01_main.ZIP`).
    Archive(String),
}

/// Metadata for one visible file.
#[derive(Clone, Debug)]
pub struct EntryInfo {
    /// Path with its original (on-disk / in-archive) casing and `/` separators.
    pub path: String,
    pub source: Source,
    /// Uncompressed size in bytes.
    pub size: u64,
}

/// Summary of a mounted scene archive.
#[derive(Clone, Debug)]
pub struct ArchiveInfo {
    /// VFS path of the ZIP file, original casing (e.g. `Scenes/M01/M01_main.ZIP`).
    pub path: String,
    pub entry_count: usize,
    pub on_disk: PathBuf,
}

struct LooseFile {
    /// Path with original casing.
    display: String,
    disk: PathBuf,
    size: u64,
}

struct Archive {
    vfs_path: String,
    on_disk: PathBuf,
    zip: Mutex<zip::ZipArchive<BufReader<File>>>,
    /// `(display name, zip index, size)` of the file entries
    entries: Vec<(String, usize, u64)>,
}

/// Lower number = searched first.
type Rank = usize;

pub struct Vfs {
    game_dir: PathBuf,
    mod_dirs: Vec<PathBuf>,
    /// One loose-file index per source: `loose[0]` is the game dir, `loose[i + 1]` is `mod_dirs[i]`.
    loose: Vec<HashMap<String, LooseFile>>,
    archives: Vec<Archive>,
    archive_rank: Vec<Rank>,
    /// normalised inner path -> `(archive id, index into Archive::entries)`
    arch_index: HashMap<String, Vec<(usize, usize)>>,
}

impl Vfs {
    /// Mounts the game directory (scans loose files, mounts every `Scenes/**.zip`).
    pub fn new(game_dir: impl Into<PathBuf>) -> Result<Vfs> {
        let game_dir = game_dir.into();
        if !game_dir.is_dir() {
            return Err(VfsError::NotFound(game_dir.display().to_string()));
        }
        let mut vfs = Vfs {
            game_dir: game_dir.clone(),
            mod_dirs: Vec::new(),
            loose: vec![scan_dir(&game_dir, true)?],
            archives: Vec::new(),
            archive_rank: Vec::new(),
            arch_index: HashMap::new(),
        };
        vfs.mount_archives_from(0)?;
        Ok(vfs)
    }

    /// Mounts `$BM_GAME_DIR` (or [`DEFAULT_GAME_DIR`]).
    pub fn from_env() -> Result<Vfs> {
        Vfs::new(game_dir_from_env())
    }

    /// Adds a mod directory. Mods added later override earlier ones; all mods override the game.
    /// Files in a mod dir are addressed relative to the dir root, e.g. `Scenes/M01/M01_main.GMS`.
    pub fn add_mod_dir(&mut self, dir: impl Into<PathBuf>) -> Result<()> {
        let dir = dir.into();
        if !dir.is_dir() {
            return Err(VfsError::NotFound(dir.display().to_string()));
        }
        let idx = self.loose.len();
        self.loose.push(scan_dir(&dir, false)?);
        self.mod_dirs.push(dir);
        self.mount_archives_from(idx)
    }

    pub fn game_dir(&self) -> &Path {
        &self.game_dir
    }

    pub fn mod_dirs(&self) -> &[PathBuf] {
        &self.mod_dirs
    }

    /// Mounted scene archives in current search order (highest priority first).
    pub fn archives(&self) -> Vec<ArchiveInfo> {
        let mut ids: Vec<usize> = (0..self.archives.len()).collect();
        ids.sort_by_key(|&i| self.archive_rank[i]);
        ids.into_iter()
            .map(|i| ArchiveInfo {
                path: self.archives[i].vfs_path.clone(),
                entry_count: self.archives[i].entries.len(),
                on_disk: self.archives[i].on_disk.clone(),
            })
            .collect()
    }

    /// Moves the named archive (VFS path, case-insensitive, e.g. `Scenes/M01/M01_main.ZIP`) to the
    /// front of the archive search order. Needed because every scene ZIP carries its own copy of
    /// `menu/*.xml` and `Shaders/**`. Returns false if no such archive exists.
    pub fn prioritize_archive(&mut self, vfs_path: &str) -> bool {
        let key = normalize(vfs_path);
        let Some(id) = self.archives.iter().position(|a| normalize(&a.vfs_path) == key) else {
            return false;
        };
        let mut order: Vec<usize> = (0..self.archives.len()).collect();
        order.sort_by_key(|&i| self.archive_rank[i]);
        order.retain(|&i| i != id);
        order.insert(0, id);
        for (r, i) in order.into_iter().enumerate() {
            self.archive_rank[i] = r;
        }
        true
    }

    /// True if `path` resolves to anything.
    pub fn exists(&self, path: &str) -> bool {
        self.stat(path).is_some()
    }

    /// On-disk path of a *loose* file (mod dir first, then game dir). `None` for archive entries.
    /// Needed for huge files that must not be read whole, e.g. `pc_eng.str`.
    pub fn disk_path(&self, path: &str) -> Option<PathBuf> {
        let key = normalize(path);
        self.source_order().find_map(|s| self.loose[s].get(&key).map(|f| f.disk.clone()))
    }

    /// Resolves a path to its winning source without reading it.
    pub fn stat(&self, path: &str) -> Option<EntryInfo> {
        let key = normalize(path);
        for src in self.source_order() {
            if let Some(f) = self.loose[src].get(&key) {
                return Some(EntryInfo {
                    path: f.display.clone(),
                    source: if src == 0 { Source::GameDir } else { Source::Mod(src - 1) },
                    size: f.size,
                });
            }
            // archives of this source tier are searched right after its loose files
            if let Some((aid, eidx)) = self.best_archive_hit(&key, src) {
                let a = &self.archives[aid];
                let e = &a.entries[eidx];
                return Some(EntryInfo { path: e.0.clone(), source: Source::Archive(a.vfs_path.clone()), size: e.2 });
            }
        }
        None
    }

    /// Reads a whole file. Loose files in mod dirs override loose game files which override archives.
    pub fn open(&self, path: &str) -> Result<Vec<u8>> {
        let key = normalize(path);
        for src in self.source_order() {
            if let Some(f) = self.loose[src].get(&key) {
                return Ok(std::fs::read(&f.disk)?);
            }
            if let Some((aid, eidx)) = self.best_archive_hit(&key, src) {
                return self.read_archive_entry(aid, eidx);
            }
        }
        Err(VfsError::NotFound(path.to_string()))
    }

    /// Reads `path` from one specific archive, ignoring overrides.
    pub fn open_in_archive(&self, archive: &str, path: &str) -> Result<Vec<u8>> {
        let akey = normalize(archive);
        let key = normalize(path);
        let aid = self
            .archives
            .iter()
            .position(|a| normalize(&a.vfs_path) == akey)
            .ok_or_else(|| VfsError::NotFound(archive.to_string()))?;
        let hits = self.arch_index.get(&key).ok_or_else(|| VfsError::NotFound(path.to_string()))?;
        let (_, eidx) = hits
            .iter()
            .find(|(a, _)| *a == aid)
            .ok_or_else(|| VfsError::NotFound(format!("{archive}:{path}")))?;
        self.read_archive_entry(aid, *eidx)
    }

    /// Lists every visible file matching `pattern` (case-insensitive glob: `*` = within one path
    /// segment, `**` = across segments, `?` = one character). Results are sorted by path and contain
    /// each logical path once, describing the *winning* source.
    pub fn list(&self, pattern: &str) -> Vec<EntryInfo> {
        let pat = normalize_glob(pattern);
        let mut seen: HashMap<String, EntryInfo> = HashMap::new();
        for src in self.source_order() {
            for (key, f) in &self.loose[src] {
                if !seen.contains_key(key) && glob::matches(&pat, key) {
                    seen.insert(
                        key.clone(),
                        EntryInfo {
                            path: f.display.clone(),
                            source: if src == 0 { Source::GameDir } else { Source::Mod(src - 1) },
                            size: f.size,
                        },
                    );
                }
            }
            for key in self.arch_index.keys() {
                if seen.contains_key(key) || !glob::matches(&pat, key) {
                    continue;
                }
                if let Some((aid, eidx)) = self.best_archive_hit(key, src) {
                    let a = &self.archives[aid];
                    let e = &a.entries[eidx];
                    seen.insert(
                        key.clone(),
                        EntryInfo { path: e.0.clone(), source: Source::Archive(a.vfs_path.clone()), size: e.2 },
                    );
                }
            }
        }
        let mut v: Vec<EntryInfo> = seen.into_values().collect();
        v.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()));
        v
    }

    /// Lists the entries of a single archive (not affected by overrides).
    pub fn list_archive(&self, archive: &str) -> Result<Vec<EntryInfo>> {
        let akey = normalize(archive);
        let a = self
            .archives
            .iter()
            .find(|a| normalize(&a.vfs_path) == akey)
            .ok_or_else(|| VfsError::NotFound(archive.to_string()))?;
        Ok(a
            .entries
            .iter()
            .map(|e| EntryInfo { path: e.0.clone(), source: Source::Archive(a.vfs_path.clone()), size: e.2 })
            .collect())
    }

    // ----- internals -----

    /// Source tiers in search order: mods last-to-first, then the game dir (index 0).
    fn source_order(&self) -> impl Iterator<Item = usize> {
        (1..self.loose.len()).rev().chain(std::iter::once(0))
    }

    /// Best archive hit for `key` among archives that belong to source tier `src`.
    fn best_archive_hit(&self, key: &str, src: usize) -> Option<(usize, usize)> {
        self.arch_index
            .get(key)?
            .iter()
            .filter(|(aid, _)| self.archive_src(*aid) == src)
            .min_by_key(|(aid, _)| self.archive_rank[*aid])
            .copied()
    }

    fn archive_src(&self, aid: usize) -> usize {
        // archives were mounted tier by tier; remember the tier via the on-disk root
        let disk = &self.archives[aid].on_disk;
        for (i, m) in self.mod_dirs.iter().enumerate().rev() {
            if disk.starts_with(m) {
                return i + 1;
            }
        }
        0
    }

    fn read_archive_entry(&self, aid: usize, eidx: usize) -> Result<Vec<u8>> {
        let a = &self.archives[aid];
        let (name, zidx, size) = &a.entries[eidx];
        let mut zip = a.zip.lock().unwrap_or_else(|p| p.into_inner());
        let mut f = zip.by_index(*zidx).map_err(|e| VfsError::Zip(format!("{}:{name}: {e}", a.vfs_path)))?;
        let mut buf = Vec::with_capacity(*size as usize);
        f.read_to_end(&mut buf)?;
        Ok(buf)
    }

    fn mount_archives_from(&mut self, src: usize) -> Result<()> {
        // collect ZIPs under Scenes/** of this tier
        let mut zips: Vec<(String, PathBuf)> = self.loose[src]
            .iter()
            .filter(|(k, _)| k.starts_with("scenes/") && k.ends_with(".zip"))
            .map(|(_, f)| (f.display.clone(), f.disk.clone()))
            .collect();
        zips.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
        for (vfs_path, disk) in zips {
            let file = File::open(&disk)?;
            let mut zip = match zip::ZipArchive::new(BufReader::new(file)) {
                Ok(z) => z,
                Err(_) => continue, // not a readable zip (e.g. a mod placeholder) - ignore
            };
            let mut entries = Vec::new();
            for i in 0..zip.len() {
                let f = zip.by_index_raw(i)?;
                if f.is_dir() {
                    continue;
                }
                entries.push((f.name().replace('\\', "/"), i, f.size()));
            }
            let aid = self.archives.len();
            for (ei, (name, _, _)) in entries.iter().enumerate() {
                self.arch_index.entry(normalize(name)).or_default().push((aid, ei));
            }
            // later-added tiers must beat earlier ones regardless of rank bookkeeping,
            // `best_archive_hit` filters by tier first so rank only orders within a tier.
            self.archive_rank.push(aid);
            self.archives.push(Archive {
                vfs_path,
                on_disk: disk,
                zip: Mutex::new(zip),
                entries,
            });
        }
        Ok(())
    }
}

fn normalize_glob(p: &str) -> String {
    // like `normalize` but keeps glob metacharacters intact (they survive lowercase/separator folding)
    normalize(p)
}

/// `$BM_GAME_DIR` or the default install path.
pub fn game_dir_from_env() -> PathBuf {
    std::env::var_os(GAME_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_GAME_DIR))
}

/// Returns the game dir if it exists; used by tests so they skip gracefully.
pub fn game_dir_if_present() -> Option<PathBuf> {
    let d = game_dir_from_env();
    if d.join("Scenes").is_dir() { Some(d) } else { None }
}

fn scan_dir(root: &Path, is_game: bool) -> Result<HashMap<String, LooseFile>> {
    let mut out = HashMap::new();
    let mut stack: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for ent in rd.flatten() {
            let name = ent.file_name().to_string_lossy().into_owned();
            let ty = match ent.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            if ty.is_dir() {
                if is_game && SKIP_DIRS.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
                    continue;
                }
                stack.push((ent.path(), rel));
            } else if ty.is_file() {
                let size = ent.metadata().map(|m| m.len()).unwrap_or(0);
                out.insert(normalize(&rel), LooseFile { display: rel, disk: ent.path(), size });
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_paths() {
        assert_eq!(normalize("Scenes\\M01/./M01_Main.GMS"), "scenes/m01/m01_main.gms");
        assert_eq!(normalize("/a//b/"), "a/b");
    }

    #[test]
    fn mod_overrides_and_glob() {
        let tmp = std::env::temp_dir().join(format!("bm_vfs_test_{}", std::process::id()));
        let game = tmp.join("game");
        let modd = tmp.join("mod");
        std::fs::create_dir_all(game.join("Scenes/X")).unwrap();
        std::fs::create_dir_all(modd.join("Scenes/X")).unwrap();
        std::fs::write(game.join("Scenes/X/a.txt"), b"game").unwrap();
        std::fs::write(game.join("b.txt"), b"bgame").unwrap();
        std::fs::write(modd.join("scenes/x/A.TXT"), b"mod").unwrap();
        let mut vfs = Vfs::new(&game).unwrap();
        assert_eq!(vfs.open("scenes/x/A.txt").unwrap(), b"game");
        vfs.add_mod_dir(&modd).unwrap();
        assert_eq!(vfs.open("Scenes/X/a.txt").unwrap(), b"mod");
        assert_eq!(vfs.open("B.TXT").unwrap(), b"bgame");
        let l = vfs.list("scenes/**/*.txt");
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].source, Source::Mod(0));
        assert!(vfs.open("nope").is_err());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn real_game_scene_zip() {
        let Some(dir) = game_dir_if_present() else {
            eprintln!("BM_GAME_DIR not available, skipping");
            return;
        };
        let mut vfs = Vfs::new(dir).unwrap();
        let archives = vfs.archives();
        assert!(archives.len() >= 10, "expected many scene zips, got {}", archives.len());
        // scene container files resolve through the archive layer, case-insensitively
        let gms = vfs.open("scenes/m01/m01_main.gms").unwrap();
        assert!(gms.len() > 100_000);
        let hits = vfs.list("Scenes/M01/M01_main.*");
        assert!(hits.iter().any(|e| e.path.to_lowercase().ends_with(".tex")));
        // per-scene shader / menu copies: prioritising picks that archive's copy
        assert!(vfs.prioritize_archive("Scenes/M01/M01_main.ZIP"));
        let st = vfs.stat("menu/ingame.xml").unwrap();
        assert_eq!(st.source, Source::Archive("Scenes/M01/M01_main.ZIP".into()));
        // raw zip is also visible as a loose file
        assert!(vfs.exists("Scenes/M01/M01_main.ZIP"));
        assert!(!vfs.list("shaders/pc/fx/*.fx").is_empty());
    }
}
