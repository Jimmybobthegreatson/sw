//! `mod.toml` manifest: parsing and validation.

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Component, Path};

/// Known permission strings. A mod only gets the Lua API modules whose permission it declares
/// (`log`, `events`, `timer`, `data`, `mod`, `mods`, `vec3` are always available).
pub const PERMISSIONS: &[&str] = &[
    "world",
    "entities.read",
    "entities.write",
    "audio",
    "fx",
    "ui",
    "console",
    "save",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
}

impl Diagnostic {
    pub fn error(m: impl Into<String>) -> Self {
        Self { severity: Severity::Error, message: m.into() }
    }
    pub fn warn(m: impl Into<String>) -> Self {
        Self { severity: Severity::Warning, message: m.into() }
    }
    pub fn info(m: impl Into<String>) -> Self {
        Self { severity: Severity::Info, message: m.into() }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScriptsSection {
    /// Entry scripts, relative to the mod root, run in order at load.
    #[serde(default)]
    pub entry: Vec<String>,
    /// Directory (relative to mod root) that `require("x")` resolves in. Default `scripts`.
    #[serde(default)]
    pub dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub license: String,
    /// Semver requirement on [`crate::API_VERSION`], e.g. `"^1.0"`. Default `"^1"`.
    #[serde(default)]
    pub api_version: Option<String>,
    /// Free text: which game build the mod was made for (informational).
    #[serde(default)]
    pub game_version: Option<String>,
    /// Higher priority loads later and therefore wins asset/data conflicts. Default 0.
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub load_after: Vec<String>,
    #[serde(default)]
    pub load_before: Vec<String>,
    #[serde(default)]
    pub conflicts: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    /// Loose asset directory mounted into the VFS. Default `assets`.
    #[serde(default)]
    pub assets_dir: Option<String>,
    /// Data patch directory (`*.json|ron|toml`). Default `data`.
    #[serde(default)]
    pub data_dir: Option<String>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub optional_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub scripts: ScriptsSection,
    /// Unknown keys (reported as warnings by `validate`).
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

impl ModManifest {
    pub fn parse(text: &str) -> Result<ModManifest, String> {
        toml::from_str(text).map_err(|e| format!("mod.toml: {e}"))
    }

    pub fn semver(&self) -> Result<Version, String> {
        Version::parse(&self.version).map_err(|e| format!("version {:?}: {e}", self.version))
    }

    pub fn api_req(&self) -> Result<VersionReq, String> {
        let s = self.api_version.as_deref().unwrap_or("^1");
        VersionReq::parse(s).map_err(|e| format!("api_version {s:?}: {e}"))
    }

    pub fn assets_dir_name(&self) -> &str {
        self.assets_dir.as_deref().unwrap_or("assets")
    }
    pub fn data_dir_name(&self) -> &str {
        self.data_dir.as_deref().unwrap_or("data")
    }
    pub fn scripts_dir_name(&self) -> &str {
        self.scripts.dir.as_deref().unwrap_or("scripts")
    }

    pub fn has_permission(&self, p: &str) -> bool {
        self.permissions.iter().any(|x| x == p)
    }

    /// Static validation (no dependency resolution). `root` is the mod directory on disk.
    pub fn validate(&self, root: &Path) -> Vec<Diagnostic> {
        let mut d = Vec::new();
        if !valid_id(&self.id) {
            d.push(Diagnostic::error(format!(
                "invalid id {:?}: use [a-z][a-z0-9_-]{{0,47}}",
                self.id
            )));
        }
        if self.name.trim().is_empty() {
            d.push(Diagnostic::error("name is empty"));
        }
        if let Err(e) = self.semver() {
            d.push(Diagnostic::error(e));
        }
        match self.api_req() {
            Err(e) => d.push(Diagnostic::error(e)),
            Ok(req) => {
                if !req.matches(&crate::api_version()) {
                    d.push(Diagnostic::error(format!(
                        "requires API {} but this engine provides {}",
                        req,
                        crate::API_VERSION
                    )));
                }
            }
        }
        for (dep, req) in self.dependencies.iter().chain(self.optional_dependencies.iter()) {
            if !valid_id(dep) {
                d.push(Diagnostic::error(format!("dependency id {dep:?} invalid")));
            }
            if VersionReq::parse(req).is_err() {
                d.push(Diagnostic::error(format!("dependency {dep}: bad version requirement {req:?}")));
            }
            if dep == &self.id {
                d.push(Diagnostic::error("mod depends on itself"));
            }
        }
        for p in &self.permissions {
            if !PERMISSIONS.contains(&p.as_str()) {
                d.push(Diagnostic::warn(format!("unknown permission {p:?}")));
            }
        }
        for k in self.extra.keys() {
            d.push(Diagnostic::warn(format!("unknown manifest key {k:?}")));
        }
        for (what, rel) in [
            ("assets_dir", self.assets_dir_name()),
            ("data_dir", self.data_dir_name()),
            ("scripts.dir", self.scripts_dir_name()),
        ] {
            if !safe_rel(rel) {
                d.push(Diagnostic::error(format!("{what} {rel:?} must be a relative path inside the mod")));
            }
        }
        for e in &self.scripts.entry {
            if !safe_rel(e) {
                d.push(Diagnostic::error(format!("entry script {e:?} must be a relative path inside the mod")));
            } else if !root.join(e).is_file() {
                d.push(Diagnostic::error(format!("entry script {e:?} not found")));
            }
        }
        if !self.scripts.entry.is_empty() && self.permissions.is_empty() {
            d.push(Diagnostic::info(
                "scripts declare no permissions: only log/events/timer/data APIs are available",
            ));
        }
        d
    }
}

pub fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 48
        && b[0].is_ascii_lowercase()
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
}

/// True when `rel` is a non-empty relative path with no `..`, root or prefix components.
pub fn safe_rel(rel: &str) -> bool {
    if rel.is_empty() {
        return false;
    }
    Path::new(rel)
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
        && !rel.contains(':')
        && !rel.starts_with('/')
        && !rel.starts_with('\\')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_validate() {
        let m = ModManifest::parse(
            "id=\"a\"\nname=\"A\"\nversion=\"1.2.3\"\nbogus=1\n[dependencies]\nb=\">=1\"\n",
        )
        .unwrap();
        let d = m.validate(Path::new("."));
        assert!(d.iter().any(|x| x.message.contains("bogus")));
        assert!(!d.iter().any(|x| x.severity == Severity::Error), "{d:?}");
    }

    #[test]
    fn bad_ids() {
        assert!(!valid_id("Bad"));
        assert!(!valid_id("../x"));
        assert!(valid_id("my-mod_2"));
        assert!(!safe_rel("../x"));
        assert!(!safe_rel("/x"));
        assert!(!safe_rel("c:/x"));
        assert!(safe_rel("a/b.lua"));
    }
}
