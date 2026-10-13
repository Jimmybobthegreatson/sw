//! Data-driven modding: `data/*.json|ron|toml` files are merge-patches (RFC 7396) applied on top of
//! base datasets that the game registers, in mod load order, validated against optional schemas.
//!
//! * dataset name = path under the mod's `data/` dir without extension, lower-cased
//!   (`data/weapons.json` -> `weapons`, `data/npc/guard.ron` -> `npc/guard`).
//! * a patch may set `"$replace": true` at its root to replace the dataset instead of merging.
//! * `null` deletes a key; arrays are replaced wholesale (RFC 7396).
//! * `data/<name>.schema.json` ships a schema for `<name>` (the game's own schema wins).
//! * A patch that makes the merged result invalid is rejected as a whole and reported.

use crate::package::{ResolvedMods, list_assets};
use crate::schema;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct DataError {
    pub mod_id: String,
    pub file: String,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct DataReport {
    /// `(mod id, dataset)` pairs successfully applied, in order.
    pub applied: Vec<(String, String)>,
    pub errors: Vec<DataError>,
}

#[derive(Debug, Clone, Default)]
pub struct DataStore {
    base: BTreeMap<String, Value>,
    schemas: BTreeMap<String, Value>,
    mod_schemas: BTreeMap<String, Value>,
    merged: BTreeMap<String, Value>,
}

/// RFC 7396 JSON merge patch.
pub fn merge_patch(target: &mut Value, patch: &Value) {
    match patch {
        Value::Object(p) => {
            if !target.is_object() {
                *target = Value::Object(Default::default());
            }
            let t = target.as_object_mut().unwrap();
            for (k, v) in p {
                if v.is_null() {
                    t.remove(k);
                } else {
                    merge_patch(t.entry(k.clone()).or_insert(Value::Null), v);
                }
            }
        }
        other => *target = other.clone(),
    }
}

/// Parse a data file by extension into a JSON value.
pub fn parse_data_file(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "json" => serde_json::from_str(&text).map_err(|e| e.to_string()),
        "toml" => toml::from_str::<Value>(&text).map_err(|e| e.to_string()),
        "ron" => {
            let v = ron::from_str::<ron::Value>(&text).map_err(|e| e.to_string())?;
            serde_json::to_value(v).map_err(|e| e.to_string())
        }
        _ => Err(format!("unsupported data extension {ext:?}")),
    }
}

impl DataStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or replace) a base dataset supplied by the game.
    pub fn set_base(&mut self, name: &str, value: Value) {
        let k = bm_vfs::normalize(name);
        self.merged.insert(k.clone(), value.clone());
        self.base.insert(k, value);
    }

    /// Register the schema for a dataset (the game's schema takes precedence over mod schemas).
    pub fn set_schema(&mut self, name: &str, schema: Value) {
        self.schemas.insert(bm_vfs::normalize(name), schema);
    }

    pub fn schema(&self, name: &str) -> Option<&Value> {
        let k = bm_vfs::normalize(name);
        self.schemas.get(&k).or_else(|| self.mod_schemas.get(&k))
    }

    /// Merged dataset (base + all mod patches).
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.merged.get(&bm_vfs::normalize(name))
    }

    /// Dotted-path lookup, e.g. `get_path("weapons", "ak.damage")`; numeric segments index arrays.
    pub fn get_path(&self, name: &str, path: &str) -> Option<&Value> {
        let mut cur = self.get(name)?;
        for seg in path.split('.').filter(|s| !s.is_empty()) {
            cur = match cur {
                Value::Object(o) => o.get(seg)?,
                Value::Array(a) => a.get(seg.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
        Some(cur)
    }

    pub fn get_as<T: DeserializeOwned>(&self, name: &str) -> Option<T> {
        serde_json::from_value(self.get(name)?.clone()).ok()
    }

    pub fn base(&self, name: &str) -> Option<&Value> {
        self.base.get(&bm_vfs::normalize(name))
    }

    pub fn names(&self) -> Vec<String> {
        self.merged.keys().cloned().collect()
    }

    /// Rebuild `merged` from the bases and the patches of every mod, in load order.
    pub fn apply_mods(&mut self, mods: &ResolvedMods) -> DataReport {
        let mut rep = DataReport::default();
        self.merged = self.base.clone();
        self.mod_schemas.clear();
        for m in &mods.order {
            let mut files: Vec<String> = list_assets(&m.data_root);
            files.sort();
            for rel in files {
                // `list_assets` lower-cases; find the real file via case-insensitive lookup.
                let Some(real) = find_case_insensitive(&m.data_root, &rel) else { continue };
                let ext = real.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
                if !matches!(ext.as_str(), "json" | "ron" | "toml") {
                    continue;
                }
                let stem = rel.rsplit_once('.').map(|x| x.0).unwrap_or(&rel).to_string();
                let err = |rep: &mut DataReport, msg: String| {
                    rep.errors.push(DataError { mod_id: m.id().to_string(), file: rel.clone(), message: msg })
                };
                let mut patch = match parse_data_file(&real) {
                    Ok(v) => v,
                    Err(e) => {
                        err(&mut rep, format!("parse error: {e}"));
                        continue;
                    }
                };
                if let Some(name) = stem.strip_suffix(".schema") {
                    self.mod_schemas.entry(name.to_string()).or_insert(patch);
                    continue;
                }
                let replace = patch
                    .as_object_mut()
                    .and_then(|o| o.remove("$replace"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let mut next = if replace { Value::Null } else { self.merged.get(&stem).cloned().unwrap_or(Value::Null) };
                merge_patch(&mut next, &patch);
                if let Some(s) = self.schema(&stem) {
                    let errs = schema::validate(s, &next);
                    if !errs.is_empty() {
                        err(&mut rep, format!("rejected by schema: {}", errs.join("; ")));
                        continue;
                    }
                }
                self.merged.insert(stem.clone(), next);
                rep.applied.push((m.id().to_string(), stem));
            }
        }
        rep
    }
}

fn find_case_insensitive(root: &Path, normalized_rel: &str) -> Option<std::path::PathBuf> {
    let mut cur = root.to_path_buf();
    for seg in normalized_rel.split('/') {
        let direct = cur.join(seg);
        if direct.exists() {
            cur = direct;
            continue;
        }
        let found = fs::read_dir(&cur)
            .ok()?
            .flatten()
            .find(|e| e.file_name().to_string_lossy().to_lowercase() == seg)?;
        cur = found.path();
    }
    Some(cur)
}

/// Cheap change detector for hot reload: hash of `(path, size, mtime)` of every data file.
pub fn data_fingerprint(mods: &ResolvedMods) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for m in &mods.order {
        m.id().hash(&mut h);
        for f in list_assets(&m.data_root) {
            if let Some(p) = find_case_insensitive(&m.data_root, &f) {
                f.hash(&mut h);
                if let Ok(md) = fs::metadata(&p) {
                    md.len().hash(&mut h);
                    if let Ok(t) = md.modified() {
                        t.hash(&mut h);
                    }
                }
            }
        }
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rfc7396() {
        let mut t = json!({"a":"b","c":{"d":"e","f":"g"},"arr":[1,2]});
        merge_patch(&mut t, &json!({"a":"z","c":{"f":null},"arr":[3],"n":{"x":null,"y":1}}));
        assert_eq!(t, json!({"a":"z","c":{"d":"e"},"arr":[3],"n":{"y":1}}));
    }
}
