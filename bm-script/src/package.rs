//! Mod discovery, enable/disable config, dependency resolution, load order, asset-conflict reports
//! and VFS mounting.

use crate::manifest::{Diagnostic, ModManifest, Severity};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ModError {
    #[error("io error on {0}: {1}")]
    Io(String, std::io::Error),
    #[error("{0}")]
    Manifest(String),
    #[error("zip error in {0}: {1}")]
    Zip(String, String),
    #[error("vfs error: {0}")]
    Vfs(#[from] bm_vfs::VfsError),
    #[error("{0}")]
    Other(String),
}

fn io_err(p: &Path) -> impl FnOnce(std::io::Error) -> ModError + '_ {
    move |e| ModError::Io(p.display().to_string(), e)
}

/// Where mods live and where state is kept.
#[derive(Debug, Clone)]
pub struct ModDirs {
    /// Scanned in order; later roots win duplicate-id ties (user dir after the install dir).
    pub roots: Vec<PathBuf>,
    /// Extraction cache for `.zip` mods.
    pub cache_dir: PathBuf,
    /// `mods.toml` (enable/disable + priority overrides).
    pub config_file: PathBuf,
    /// Per-mod persistent JSON save data.
    pub save_dir: PathBuf,
}

impl ModDirs {
    /// `<exe dir>/Mods` and `<user dir>/Mods`; state in the user dir.
    /// User dir: `$BM_USER_DIR`, else `%APPDATA%/bm-rs`, else `$HOME/.bm-rs`.
    pub fn default_dirs() -> ModDirs {
        let user = user_dir();
        let mut roots = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(d) = exe.parent() {
                roots.push(d.join("Mods"));
            }
        }
        roots.push(user.join("Mods"));
        ModDirs::with_state_dir(roots, &user)
    }

    pub fn with_state_dir(roots: Vec<PathBuf>, state: &Path) -> ModDirs {
        ModDirs {
            roots,
            cache_dir: state.join("mod_cache"),
            config_file: state.join("mods.toml"),
            save_dir: state.join("mod_saves"),
        }
    }
}

pub fn user_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("BM_USER_DIR") {
        return PathBuf::from(d);
    }
    if let Some(d) = std::env::var_os("APPDATA") {
        return PathBuf::from(d).join("bm-rs");
    }
    if let Some(d) = std::env::var_os("HOME") {
        return PathBuf::from(d).join(".bm-rs");
    }
    PathBuf::from(".bm-rs")
}

/// User-editable enable/disable list (`mods.toml`).
///
/// ```toml
/// disabled = ["some_mod"]
/// [priority]
/// other_mod = 10
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModConfig {
    #[serde(default)]
    pub disabled: BTreeSet<String>,
    /// Overrides `priority` from the manifest.
    #[serde(default)]
    pub priority: BTreeMap<String, i32>,
}

impl ModConfig {
    pub fn load(path: &Path) -> Result<ModConfig, ModError> {
        match fs::read_to_string(path) {
            Ok(t) => toml::from_str(&t).map_err(|e| ModError::Manifest(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ModConfig::default()),
            Err(e) => Err(io_err(path)(e)),
        }
    }
    pub fn save(&self, path: &Path) -> Result<(), ModError> {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).map_err(io_err(p))?;
        }
        let t = toml::to_string_pretty(self).map_err(|e| ModError::Other(e.to_string()))?;
        fs::write(path, t).map_err(io_err(path))
    }
    pub fn is_enabled(&self, id: &str) -> bool {
        !self.disabled.contains(id)
    }
    pub fn set_enabled(&mut self, id: &str, on: bool) {
        if on {
            self.disabled.remove(id);
        } else {
            self.disabled.insert(id.to_string());
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModSource {
    Dir(PathBuf),
    Zip(PathBuf),
}

#[derive(Debug, Clone)]
pub struct DiscoveredMod {
    pub manifest: ModManifest,
    /// Directory on disk that holds `mod.toml` (the extraction dir for zips).
    pub root: PathBuf,
    pub source: ModSource,
    pub diagnostics: Vec<Diagnostic>,
}

impl DiscoveredMod {
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| d.severity == Severity::Error)
    }
    pub fn version(&self) -> Version {
        self.manifest.semver().unwrap_or_else(|_| Version::new(0, 0, 0))
    }
}

#[derive(Debug, Clone)]
pub struct DiscoveryFailure {
    pub path: PathBuf,
    pub error: String,
}

// ------------------------------------------------------------------------------------------
// discovery
// ------------------------------------------------------------------------------------------

const MAX_ZIP_FILES: usize = 20_000;
const MAX_ZIP_BYTES: u64 = 1 << 30;

/// Extract a mod zip into `cache_dir`, reusing a previous extraction when size+mtime match.
/// Returns the directory that contains `mod.toml`.
pub fn extract_zip_mod(zip_path: &Path, cache_dir: &Path) -> Result<PathBuf, ModError> {
    let meta = fs::metadata(zip_path).map_err(io_err(zip_path))?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let stem = zip_path.file_stem().and_then(|s| s.to_str()).unwrap_or("mod");
    let dest = cache_dir.join(format!("{stem}-{}-{mtime}", meta.len()));
    let ok_marker = dest.join(".extracted");
    if !ok_marker.exists() {
        let zerr = |e: &dyn std::fmt::Display| ModError::Zip(zip_path.display().to_string(), e.to_string());
        let f = fs::File::open(zip_path).map_err(io_err(zip_path))?;
        let mut ar = zip::ZipArchive::new(f).map_err(|e| zerr(&e))?;
        if ar.len() > MAX_ZIP_FILES {
            return Err(zerr(&"too many entries"));
        }
        if dest.exists() {
            let _ = fs::remove_dir_all(&dest);
        }
        fs::create_dir_all(&dest).map_err(io_err(&dest))?;
        let mut total = 0u64;
        for i in 0..ar.len() {
            let mut e = ar.by_index(i).map_err(|e| zerr(&e))?;
            // zip-slip protection: enclosed_name rejects absolute paths and `..`.
            let Some(rel) = e.enclosed_name() else {
                return Err(zerr(&format!("unsafe entry path {:?}", e.name())));
            };
            let out = dest.join(rel);
            if e.is_dir() {
                fs::create_dir_all(&out).map_err(io_err(&out))?;
                continue;
            }
            total = total.saturating_add(e.size());
            if total > MAX_ZIP_BYTES {
                return Err(zerr(&"archive too large when extracted"));
            }
            if let Some(p) = out.parent() {
                fs::create_dir_all(p).map_err(io_err(p))?;
            }
            let mut w = fs::File::create(&out).map_err(io_err(&out))?;
            // Read at most declared size + 1 (guards against lying headers).
            let mut limited = (&mut e).take(MAX_ZIP_BYTES);
            std::io::copy(&mut limited, &mut w).map_err(io_err(&out))?;
        }
        fs::write(&ok_marker, b"ok").map_err(io_err(&ok_marker))?;
    }
    if dest.join("mod.toml").is_file() {
        return Ok(dest);
    }
    // Single top-level folder containing mod.toml.
    let mut dirs = Vec::new();
    for e in fs::read_dir(&dest).map_err(io_err(&dest))?.flatten() {
        if e.path().is_dir() {
            dirs.push(e.path());
        }
    }
    if dirs.len() == 1 && dirs[0].join("mod.toml").is_file() {
        return Ok(dirs.remove(0));
    }
    Err(ModError::Manifest(format!("{}: no mod.toml at archive root", zip_path.display())))
}

fn load_mod_at(root: &Path, source: ModSource) -> Result<DiscoveredMod, ModError> {
    let mf = root.join("mod.toml");
    let text = fs::read_to_string(&mf).map_err(io_err(&mf))?;
    let manifest = ModManifest::parse(&text).map_err(ModError::Manifest)?;
    let diagnostics = manifest.validate(root);
    Ok(DiscoveredMod { manifest, root: root.to_path_buf(), source, diagnostics })
}

/// Load a single mod folder or `.zip` (zips are extracted into `cache_dir`).
pub fn load_mod(path: &Path, cache_dir: &Path) -> Result<DiscoveredMod, ModError> {
    if path.is_dir() {
        load_mod_at(path, ModSource::Dir(path.to_path_buf()))
    } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")) {
        let root = extract_zip_mod(path, cache_dir)?;
        load_mod_at(&root, ModSource::Zip(path.to_path_buf()))
    } else {
        Err(ModError::Other(format!("{}: not a directory or .zip", path.display())))
    }
}

/// Everything found in the mod roots plus problems with duplicates/unreadable mods.
#[derive(Debug, Clone, Default)]
pub struct Discovery {
    pub mods: Vec<DiscoveredMod>,
    pub failures: Vec<DiscoveryFailure>,
    pub problems: Vec<Problem>,
}

pub fn discover(dirs: &ModDirs) -> Discovery {
    let mut out = Discovery::default();
    let mut by_id: HashMap<String, usize> = HashMap::new();
    for root in &dirs.roots {
        let Ok(rd) = fs::read_dir(root) else { continue };
        let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        paths.sort();
        for p in paths {
            let is_zip = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"));
            if !(is_zip || p.join("mod.toml").is_file()) {
                continue;
            }
            match load_mod(&p, &dirs.cache_dir) {
                Ok(m) => {
                    let id = m.manifest.id.clone();
                    if let Some(&prev) = by_id.get(&id) {
                        // Keep the higher version; ties go to the later root.
                        let keep_new = m.version() >= out.mods[prev].version();
                        let (loser_path, kept) = if keep_new {
                            (out.mods[prev].root.clone(), m.root.display().to_string())
                        } else {
                            (m.root.clone(), out.mods[prev].root.display().to_string())
                        };
                        out.problems.push(Problem {
                            severity: Severity::Warning,
                            mod_id: Some(id.clone()),
                            kind: ProblemKind::DuplicateId,
                            message: format!(
                                "duplicate mod id {id:?}: ignoring {} in favour of {kept}",
                                loser_path.display()
                            ),
                        });
                        if keep_new {
                            out.mods[prev] = m;
                        }
                    } else {
                        by_id.insert(id, out.mods.len());
                        out.mods.push(m);
                    }
                }
                Err(e) => out.failures.push(DiscoveryFailure { path: p, error: e.to_string() }),
            }
        }
    }
    out
}

// ------------------------------------------------------------------------------------------
// resolution
// ------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ProblemKind {
    Invalid,
    DuplicateId,
    DisabledByUser,
    MissingDependency,
    DependencyVersion,
    Conflict,
    Cycle,
    OptionalDependencyVersion,
}

#[derive(Debug, Clone, Serialize)]
pub struct Problem {
    pub severity: Severity,
    pub mod_id: Option<String>,
    pub kind: ProblemKind,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct ResolvedMod {
    pub manifest: ModManifest,
    pub root: PathBuf,
    pub source: ModSource,
    pub assets_root: PathBuf,
    pub data_root: PathBuf,
    pub priority: i32,
}

impl ResolvedMod {
    pub fn id(&self) -> &str {
        &self.manifest.id
    }
}

/// Result of dependency resolution: `order` is the load order (index 0 loads first, so the last
/// mod wins conflicts), `problems` lists everything that was skipped and why.
#[derive(Debug, Clone, Default)]
pub struct ResolvedMods {
    pub order: Vec<ResolvedMod>,
    pub problems: Vec<Problem>,
}

impl ResolvedMods {
    pub fn ids(&self) -> Vec<&str> {
        self.order.iter().map(|m| m.id()).collect()
    }
    pub fn get(&self, id: &str) -> Option<&ResolvedMod> {
        self.order.iter().find(|m| m.id() == id)
    }
    pub fn has_errors(&self) -> bool {
        self.problems.iter().any(|p| p.severity == Severity::Error)
    }
}

/// Pure resolution of a set of discovered mods (no filesystem access).
pub fn resolve(mods: &[DiscoveredMod], config: &ModConfig) -> ResolvedMods {
    let mut problems = Vec::new();
    let mut active: BTreeMap<String, &DiscoveredMod> = BTreeMap::new();
    for m in mods {
        let id = &m.manifest.id;
        if m.has_errors() {
            let msgs: Vec<_> = m
                .diagnostics
                .iter()
                .filter(|d| d.severity == Severity::Error)
                .map(|d| d.message.as_str())
                .collect();
            problems.push(Problem {
                severity: Severity::Error,
                mod_id: Some(id.clone()),
                kind: ProblemKind::Invalid,
                message: format!("{id}: invalid manifest: {}", msgs.join("; ")),
            });
            continue;
        }
        if !config.is_enabled(id) {
            problems.push(Problem {
                severity: Severity::Info,
                mod_id: Some(id.clone()),
                kind: ProblemKind::DisabledByUser,
                message: format!("{id}: disabled by user"),
            });
            continue;
        }
        active.insert(id.clone(), m);
    }
    let prio = |m: &DiscoveredMod| config.priority.get(&m.manifest.id).copied().unwrap_or(m.manifest.priority);

    let order_ids = loop {
        // Fixpoint: remove mods with unmet hard dependencies / conflicts.
        loop {
            let mut remove: Vec<(String, Problem)> = Vec::new();
            for (id, m) in &active {
                for (dep, req) in &m.manifest.dependencies {
                    match active.get(dep) {
                        None => remove.push((
                            id.clone(),
                            Problem {
                                severity: Severity::Error,
                                mod_id: Some(id.clone()),
                                kind: ProblemKind::MissingDependency,
                                message: format!("{id}: missing or disabled dependency {dep:?} ({req})"),
                            },
                        )),
                        Some(d) => {
                            let ok = VersionReq::parse(req).map(|r| r.matches(&d.version())).unwrap_or(false);
                            if !ok {
                                remove.push((
                                    id.clone(),
                                    Problem {
                                        severity: Severity::Error,
                                        mod_id: Some(id.clone()),
                                        kind: ProblemKind::DependencyVersion,
                                        message: format!(
                                            "{id}: dependency {dep} {} does not satisfy {req}",
                                            d.manifest.version
                                        ),
                                    },
                                ));
                            }
                        }
                    }
                }
                for c in &m.manifest.conflicts {
                    if let Some(o) = active.get(c) {
                        // The lower-priority mod loses; ties: the later id alphabetically.
                        let loser = if (prio(m), id.as_str()) < (prio(o), c.as_str()) { id } else { c };
                        let winner = if loser == id { c } else { id };
                        remove.push((
                            loser.clone(),
                            Problem {
                                severity: Severity::Error,
                                mod_id: Some(loser.clone()),
                                kind: ProblemKind::Conflict,
                                message: format!("{loser}: conflicts with {winner}; {loser} was disabled"),
                            },
                        ));
                    }
                }
            }
            if remove.is_empty() {
                break;
            }
            for (id, p) in remove {
                if active.remove(&id).is_some() {
                    problems.push(p);
                }
            }
        }

        // Edges: a -> b means a loads before b.
        let mut succ: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut indeg: BTreeMap<String, usize> = active.keys().map(|k| (k.clone(), 0)).collect();
        let mut edges: Vec<(String, String)> = Vec::new();
        for (id, m) in &active {
            for dep in m.manifest.dependencies.keys() {
                edges.push((dep.clone(), id.clone()));
            }
            for (dep, req) in &m.manifest.optional_dependencies {
                if let Some(d) = active.get(dep) {
                    if VersionReq::parse(req).map(|r| r.matches(&d.version())).unwrap_or(false) {
                        edges.push((dep.clone(), id.clone()));
                    }
                }
            }
            for o in &m.manifest.load_after {
                edges.push((o.clone(), id.clone()));
            }
            for o in &m.manifest.load_before {
                edges.push((id.clone(), o.clone()));
            }
        }
        for (a, b) in edges {
            if !active.contains_key(&a) || !active.contains_key(&b) {
                continue;
            }
            if succ.entry(a).or_default().insert(b.clone()) {
                *indeg.get_mut(&b).unwrap() += 1;
            }
        }
        // Kahn with deterministic tie-break: lowest (priority, id) first.
        let mut ready: BTreeSet<(i32, String)> =
            indeg.iter().filter(|(_, d)| **d == 0).map(|(k, _)| (prio(active[k]), k.clone())).collect();
        let mut order: Vec<String> = Vec::new();
        while let Some(first) = ready.iter().next().cloned() {
            ready.remove(&first);
            if let Some(ss) = succ.get(&first.1) {
                for s in ss {
                    let d = indeg.get_mut(s).unwrap();
                    *d -= 1;
                    if *d == 0 {
                        ready.insert((prio(active[s]), s.clone()));
                    }
                }
            }
            order.push(first.1);
        }
        if order.len() == active.len() {
            break order;
        }
        // Cycle: peel nodes with no remaining successors to isolate the cyclic core.
        let mut left: BTreeSet<String> = active.keys().filter(|k| !order.contains(k)).cloned().collect();
        loop {
            let leaf: Vec<String> = left
                .iter()
                .filter(|n| succ.get(*n).is_none_or(|s| s.iter().all(|x| !left.contains(x))))
                .cloned()
                .collect();
            if leaf.is_empty() {
                break;
            }
            for l in leaf {
                left.remove(&l);
            }
        }
        let cyc: Vec<String> = left.into_iter().collect();
        problems.push(Problem {
            severity: Severity::Error,
            mod_id: None,
            kind: ProblemKind::Cycle,
            message: format!("dependency/load-order cycle between: {}", cyc.join(", ")),
        });
        if cyc.is_empty() {
            // Should not happen; avoid an infinite loop.
            break order;
        }
        for c in cyc {
            active.remove(&c);
        }
    };

    let order = order_ids
        .into_iter()
        .map(|id| {
            let m = active[&id];
            ResolvedMod {
                manifest: m.manifest.clone(),
                root: m.root.clone(),
                source: m.source.clone(),
                assets_root: m.root.join(m.manifest.assets_dir_name()),
                data_root: m.root.join(m.manifest.data_dir_name()),
                priority: prio(m),
            }
        })
        .collect::<Vec<_>>();

    // Soft warnings: optional deps present but wrong version.
    for m in &order {
        for (dep, req) in &m.manifest.optional_dependencies {
            if let Some(d) = active.get(dep) {
                if !VersionReq::parse(req).map(|r| r.matches(&d.version())).unwrap_or(false) {
                    problems.push(Problem {
                        severity: Severity::Warning,
                        mod_id: Some(m.id().to_string()),
                        kind: ProblemKind::OptionalDependencyVersion,
                        message: format!("{}: optional dependency {dep} {} does not satisfy {req}", m.id(), d.manifest.version),
                    });
                }
            }
        }
    }
    ResolvedMods { order, problems }
}

// ------------------------------------------------------------------------------------------
// conflicts
// ------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct AssetConflict {
    /// Normalised VFS path (lower case, `/`).
    pub path: String,
    /// Providing mods in load order; the last one wins.
    pub mods: Vec<String>,
    pub winner: String,
    /// `Some(true)` when the game's own files also contain this path.
    pub overrides_game: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ConflictReport {
    /// Paths provided by two or more mods.
    pub conflicts: Vec<AssetConflict>,
    /// Number of files each mod provides.
    pub files_per_mod: BTreeMap<String, usize>,
    /// Paths where a mod replaces a base-game file (only filled when a `Vfs` was supplied).
    pub game_overrides: BTreeMap<String, Vec<String>>,
}

/// List every file under `dir`, keyed by normalised VFS path.
pub fn list_assets(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, cur: &Path, out: &mut Vec<String>) {
        let Ok(rd) = fs::read_dir(cur) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(base, &p, out);
            } else if let Ok(rel) = p.strip_prefix(base) {
                out.push(bm_vfs::normalize(&rel.to_string_lossy()));
            }
        }
    }
    let mut v = Vec::new();
    walk(dir, dir, &mut v);
    v.sort();
    v
}

/// Report which mods provide the same asset path. Pass a `Vfs` *without* mods mounted to also learn
/// which paths override game files.
pub fn asset_conflicts(set: &ResolvedMods, vfs: Option<&bm_vfs::Vfs>) -> ConflictReport {
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut rep = ConflictReport::default();
    for m in &set.order {
        let files = list_assets(&m.assets_root);
        rep.files_per_mod.insert(m.id().to_string(), files.len());
        for f in files {
            map.entry(f).or_default().push(m.id().to_string());
        }
    }
    for (path, mods) in map {
        let og = vfs.map(|v| v.stat(&path).is_some());
        if og == Some(true) {
            for m in &mods {
                rep.game_overrides.entry(m.clone()).or_default().push(path.clone());
            }
        }
        if mods.len() > 1 {
            rep.conflicts.push(AssetConflict {
                winner: mods.last().cloned().unwrap_or_default(),
                path,
                mods,
                overrides_game: og,
            });
        }
    }
    rep
}

// ------------------------------------------------------------------------------------------
// manager facade
// ------------------------------------------------------------------------------------------

/// Convenience facade over discovery + config + resolution: the "bm mods" API.
#[derive(Debug, Clone)]
pub struct ModManager {
    pub dirs: ModDirs,
    pub config: ModConfig,
    pub discovery: Discovery,
}

#[derive(Debug, Clone)]
pub struct MountReport {
    /// Mod ids mounted, in order (later wins).
    pub mounted: Vec<String>,
    pub conflicts: ConflictReport,
}

impl ModManager {
    pub fn new(dirs: ModDirs) -> Result<ModManager, ModError> {
        let config = ModConfig::load(&dirs.config_file)?;
        let discovery = discover(&dirs);
        Ok(ModManager { dirs, config, discovery })
    }

    pub fn rescan(&mut self) {
        self.discovery = discover(&self.dirs);
    }

    pub fn mods(&self) -> &[DiscoveredMod] {
        &self.discovery.mods
    }

    pub fn resolve(&self) -> ResolvedMods {
        let mut r = resolve(&self.discovery.mods, &self.config);
        let mut p = self.discovery.problems.clone();
        p.append(&mut r.problems);
        r.problems = p;
        r
    }

    pub fn set_enabled(&mut self, id: &str, on: bool) -> Result<(), ModError> {
        if !self.discovery.mods.iter().any(|m| m.manifest.id == id) {
            return Err(ModError::Other(format!("unknown mod {id:?}")));
        }
        self.config.set_enabled(id, on);
        self.config.save(&self.dirs.config_file)
    }

    /// Mount every resolved mod's asset dir into `vfs` in load order and return the conflict report.
    pub fn mount(&self, vfs: &mut bm_vfs::Vfs, set: &ResolvedMods) -> Result<MountReport, ModError> {
        let base = if vfs.mod_dirs().is_empty() { Some(&*vfs) } else { None };
        let conflicts = asset_conflicts(set, base);
        let mut mounted = Vec::new();
        for m in &set.order {
            if m.assets_root.is_dir() {
                vfs.add_mod_dir(&m.assets_root)?;
                mounted.push(m.id().to_string());
            }
        }
        Ok(MountReport { mounted, conflicts })
    }

    /// Human-readable listing (`bm mods list`).
    pub fn list_text(&self) -> String {
        let set = self.resolve();
        let mut s = String::new();
        let order: HashMap<&str, usize> = set.order.iter().enumerate().map(|(i, m)| (m.id(), i)).collect();
        for m in &self.discovery.mods {
            let id = &m.manifest.id;
            let state = if let Some(i) = order.get(id.as_str()) {
                format!("enabled #{}", i + 1)
            } else if !self.config.is_enabled(id) {
                "disabled".to_string()
            } else {
                "skipped (see problems)".to_string()
            };
            s.push_str(&format!(
                "{:<24} {:<10} {:<20} {}\n",
                id,
                m.manifest.version,
                state,
                m.manifest.name
            ));
        }
        for f in &self.discovery.failures {
            s.push_str(&format!("FAILED {}: {}\n", f.path.display(), f.error));
        }
        if s.is_empty() {
            s.push_str("no mods found\n");
        }
        s
    }
}
