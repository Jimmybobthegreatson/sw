//! Localisation: string-id lookup over scene `.LOC` trees and mod string tables, plus the LOC mini
//! markup (`{locale}path{endlocale}`, `{if VAR more N}..{else}..{endif}`, `{Var}`).
//!
//! Lookup order for a key such as `Frontend/StartGame`:
//! 1. mod string tables (`Localisation/<lang>.json|ron`, then `Localisation/en.*`) - flat `key -> text` maps,
//!    keys may be relative (`Frontend/StartGame`) or absolute (`AllLevels/Interface/Frontend/StartGame`);
//! 2. every loaded scene [`Loc`] tree, trying the key under each *root*: `AllLevels/Interface/` (menu XML),
//!    `AllLevels/` (HUD / info texts), and verbatim (`M01/...`, `NewsPaper/...`);
//! 3. fallback: the text is `?` + key (or the key when the key looks like a literal sentence).
//!
//! A key starting with `%` is a literal (the XML uses `%SaveAndContiune (debug)` for debug entries).

use std::collections::HashMap;

use bm_formats::loc::Loc;
use bm_vfs::Vfs;

#[derive(Clone, Debug)]
enum Layer {
    Loc(Loc),
    Table(HashMap<String, String>),
}

#[derive(Clone, Debug)]
pub struct Strings {
    layers: Vec<Layer>,
    roots: Vec<String>,
    /// Current language tag (`en`, `fr`, ..): selects the mod tables.
    pub language: String,
    /// Variables for `{Var}` and `{if Var ..}` (keys lower-cased).
    vars: HashMap<String, String>,
}

impl Default for Strings {
    fn default() -> Self {
        Strings::new()
    }
}

impl Strings {
    pub fn new() -> Strings {
        Strings {
            layers: Vec::new(),
            roots: vec!["AllLevels/Interface/".into(), "AllLevels/".into(), String::new()],
            language: "en".into(),
            vars: HashMap::new(),
        }
    }

    pub fn with_loc(loc: Loc) -> Strings {
        let mut s = Strings::new();
        s.add_loc(loc);
        s
    }

    /// Appends a scene LOC tree (searched after the mod tables and earlier LOCs).
    pub fn add_loc(&mut self, loc: Loc) {
        self.layers.push(Layer::Loc(loc));
    }

    /// Adds a flat table with the highest priority.
    pub fn add_table(&mut self, table: HashMap<String, String>) {
        let t = table.into_iter().map(|(k, v)| (k.to_ascii_lowercase(), v)).collect();
        // mod tables go before LOC layers but after earlier mod tables (later add = lower priority than first)
        let pos = self.layers.iter().take_while(|l| matches!(l, Layer::Table(_))).count();
        self.layers.insert(pos, Layer::Table(t));
    }

    pub fn set_roots(&mut self, roots: Vec<String>) {
        self.roots = roots;
    }

    pub fn set_var(&mut self, k: &str, v: impl ToString) {
        self.vars.insert(k.to_ascii_lowercase(), v.to_string());
    }

    pub fn var(&self, k: &str) -> Option<&str> {
        self.vars.get(&k.to_ascii_lowercase()).map(String::as_str)
    }

    /// Loads `Localisation/<lang>.{json,ron}` and `Localisation/<lang>/*.{json,ron}` from the VFS, then the same
    /// for `en` as a fallback language. Returns the number of tables loaded.
    pub fn load_mod_tables(&mut self, vfs: &Vfs) -> usize {
        self.layers.retain(|l| !matches!(l, Layer::Table(_)));
        let mut langs = vec![self.language.clone()];
        if self.language != "en" {
            langs.push("en".into());
        }
        let mut n = 0;
        let mut tables = Vec::new();
        for lang in &langs {
            let mut paths = vec![format!("Localisation/{lang}.json"), format!("Localisation/{lang}.ron")];
            let mut dir: Vec<String> = vfs
                .list(&format!("Localisation/{lang}/*"))
                .into_iter()
                .map(|e| e.path)
                .filter(|p| p.to_ascii_lowercase().ends_with(".json") || p.to_ascii_lowercase().ends_with(".ron"))
                .collect();
            dir.sort();
            paths.extend(dir);
            for p in paths {
                let Ok(bytes) = vfs.open(&p) else { continue };
                match parse_table(&p, &crate::xml::decode_text(&bytes)) {
                    Ok(t) => {
                        tables.push(t);
                        n += 1;
                    }
                    Err(e) => log::warn!("string table {p}: {e}"),
                }
            }
        }
        // first loaded = highest priority
        for (i, t) in tables.into_iter().enumerate() {
            let t = t.into_iter().map(|(k, v)| (k.to_ascii_lowercase(), v)).collect();
            self.layers.insert(i, Layer::Table(t));
        }
        n
    }

    /// Looks `key` up without fallback.
    pub fn lookup(&self, key: &str) -> Option<String> {
        let key = key.trim();
        for l in &self.layers {
            match l {
                Layer::Table(t) => {
                    let lk = key.to_ascii_lowercase();
                    if let Some(v) = t.get(&lk) {
                        return Some(v.clone());
                    }
                    for r in &self.roots {
                        if let Some(v) = t.get(&format!("{r}{lk}").to_ascii_lowercase()) {
                            return Some(v.clone());
                        }
                    }
                }
                Layer::Loc(loc) => {
                    for r in &self.roots {
                        if let Some(v) = loc.get(&format!("{r}{key}")) {
                            return Some(v);
                        }
                    }
                }
            }
        }
        None
    }

    pub fn contains(&self, key: &str) -> bool {
        self.lookup(key).is_some()
    }

    /// Display text for a key: literal (`%..`), localised, or fallback (`?key?` marks a missing id, but keys
    /// that already look like prose - contain a space - pass through unchanged).
    pub fn text(&self, key: &str) -> String {
        if let Some(lit) = key.strip_prefix('%') {
            return lit.to_string();
        }
        if key.is_empty() {
            return String::new();
        }
        match self.lookup(key) {
            Some(t) => t,
            None if key.contains(' ') => key.to_string(),
            None => format!("?{key}?"),
        }
    }

    /// Lenient variant for hand-written layouts (HUD, mod screens): `%literal`, a localised key, or - when no such
    /// key exists - the string itself (no `?key?` marker), then markup expansion.
    pub fn resolve(&self, s: &str) -> String {
        if let Some(lit) = s.strip_prefix('%') {
            return self.expand(lit);
        }
        match self.lookup(s) {
            Some(t) => self.expand(&t),
            None => self.expand(s),
        }
    }

    /// [`Strings::text`] followed by markup expansion (`{locale}`, `{if}`, `{Var}`).
    pub fn expand_key(&self, key: &str) -> String {
        self.expand(&self.text(key))
    }

    /// Expands `{locale}..{endlocale}`, `{if}/{else}/{endif}` and `{Var}` (variables set with `set_var`).
    /// Unknown `{Token}`s (button glyphs such as `{ZW_SELECT}`) are left for the text layout stage.
    pub fn expand(&self, s: &str) -> String {
        self.expand_depth(s, 0)
    }

    fn expand_depth(&self, s: &str, depth: u32) -> String {
        if depth > 8 || !s.contains('{') {
            return s.to_string();
        }
        let mut out = String::with_capacity(s.len());
        // frames: (enclosing context active, condition value, inside else branch)
        let mut stack: Vec<(bool, bool, bool)> = Vec::new();
        let active = |st: &Vec<(bool, bool, bool)>| st.last().map(|&(p, c, e)| p && (c != e)).unwrap_or(true);
        let mut rest = s;
        while let Some(open) = rest.find('{') {
            if active(&stack) {
                out.push_str(&rest[..open]);
            }
            rest = &rest[open..];
            let Some(close) = rest.find('}') else {
                if active(&stack) {
                    out.push_str(rest);
                }
                rest = "";
                break;
            };
            let tag = &rest[1..close];
            let tl = tag.trim().to_ascii_lowercase();
            if tl == "locale" {
                let after = &rest[close + 1..];
                if let Some(end) = after.to_ascii_lowercase().find("{endlocale}") {
                    if active(&stack) {
                        let key = after[..end].trim();
                        let text = self.lookup(key).unwrap_or_else(|| format!("?{key}?"));
                        out.push_str(&self.expand_depth(&text, depth + 1));
                    }
                    rest = &after[end + "{endlocale}".len()..];
                    continue;
                }
            } else if let Some(cond) = tl.strip_prefix("if ") {
                let parent = active(&stack);
                stack.push((parent, self.eval_condition(cond), false));
            } else if tl == "else" {
                if let Some(top) = stack.last_mut() {
                    top.2 = true;
                }
            } else if tl == "endif" {
                stack.pop();
            } else if let Some(v) = self.vars.get(&tl) {
                if active(&stack) {
                    out.push_str(v);
                }
            } else if active(&stack) {
                out.push_str(&rest[..close + 1]); // keep unknown token
            }
            rest = &rest[close + 1..];
        }
        if active(&stack) {
            out.push_str(rest);
        }
        out
    }

    fn operand(&self, tok: &str) -> Option<f64> {
        tok.parse::<f64>().ok().or_else(|| self.vars.get(tok).and_then(|v| v.trim().parse().ok()))
    }

    /// `{if Var more|less|equal|not equal Value}` (value: number or another variable).
    fn eval_condition(&self, cond: &str) -> bool {
        let toks: Vec<&str> = cond.split_whitespace().collect();
        if toks.len() < 3 {
            return self.vars.get(*toks.first().unwrap_or(&"")).is_some_and(|v| v != "0" && !v.is_empty());
        }
        let lhs = self.operand(toks[0]);
        let (op, rhs_tok) = if toks[1] == "not" && toks.len() >= 4 { ("not equal", toks[3]) } else { (toks[1], toks[2]) };
        let (Some(l), Some(r)) = (lhs, self.operand(rhs_tok)) else {
            // string comparison fallback
            let l = self.vars.get(toks[0]).map(String::as_str).unwrap_or("");
            return match op {
                "equal" => l == rhs_tok,
                "not equal" => l != rhs_tok,
                _ => false,
            };
        };
        match op {
            "more" => l > r,
            "less" => l < r,
            "equal" => (l - r).abs() < 1e-9,
            "not equal" => (l - r).abs() >= 1e-9,
            _ => false,
        }
    }
}

fn parse_table(path: &str, text: &str) -> Result<HashMap<String, String>, String> {
    if path.to_ascii_lowercase().ends_with(".ron") {
        ron::from_str::<HashMap<String, String>>(text).map_err(|e| e.to_string())
    } else {
        serde_json::from_str::<HashMap<String, String>>(text).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bm_formats::loc::{Leaf, LocBody, LocNode};

    fn leaf(k: &str, t: &str) -> LocNode {
        let mut payload = t.as_bytes().to_vec();
        payload.extend_from_slice(&[0; 5]);
        LocNode { key: k.into(), body: LocBody::Leaf(Leaf { flag: 0x09, payload }) }
    }
    fn group(k: &str, c: Vec<LocNode>) -> LocNode {
        LocNode { key: k.into(), body: LocBody::Group(c) }
    }
    fn sample() -> Strings {
        let loc = Loc {
            top: vec![
                group(
                    "AllLevels",
                    vec![
                        group("Interface", vec![group("Frontend", vec![leaf("StartGame", "START GAME")])]),
                        group("Info", vec![leaf("Hint", "Hint:")]),
                    ],
                ),
                group("M01", vec![leaf("Title", "Hacienda {Name}")]),
            ],
        };
        Strings::with_loc(loc)
    }

    #[test]
    fn lookup_roots_and_fallback() {
        let s = sample();
        assert_eq!(s.text("Frontend/StartGame"), "START GAME");
        assert_eq!(s.text("frontend/startgame"), "START GAME");
        assert_eq!(s.text("Info/Hint"), "Hint:");
        assert_eq!(s.text("M01/Title"), "Hacienda {Name}");
        assert_eq!(s.text("Nope/Missing"), "?Nope/Missing?");
        assert_eq!(s.text("%literal text"), "literal text");
        assert_eq!(s.text("two words"), "two words");
        assert_eq!(s.resolve("Frontend/StartGame"), "START GAME");
        assert_eq!(s.resolve("Plain {Name}"), "Plain {Name}");
        assert_eq!(s.resolve("%{Name}"), "{Name}");
        assert_eq!(s.resolve("Nope"), "Nope");
    }

    #[test]
    fn mod_table_overrides_and_has_priority() {
        let mut s = sample();
        let mut t = HashMap::new();
        t.insert("Frontend/StartGame".to_string(), "NEW GAME".to_string());
        t.insert("AllLevels/Interface/Frontend/Extra".to_string(), "EXTRA".to_string());
        s.add_table(t);
        assert_eq!(s.text("Frontend/StartGame"), "NEW GAME");
        assert_eq!(s.text("Frontend/Extra"), "EXTRA");
        assert_eq!(s.text("Info/Hint"), "Hint:");
    }

    #[test]
    fn markup_locale_if_vars() {
        let mut s = sample();
        s.set_var("Name", "Estate");
        assert_eq!(s.expand_key("M01/Title"), "Hacienda Estate");
        assert_eq!(s.expand("a {locale}Info/Hint{endlocale} b"), "a Hint: b");
        s.set_var("Shots", 5);
        assert_eq!(s.expand("{if Shots more 1}many{else}few{endif}"), "many");
        assert_eq!(s.expand("{if Shots less 1}many{else}few{endif}"), "few");
        assert_eq!(s.expand("{if Shots equal 5}eq{endif}!"), "eq!");
        assert_eq!(s.expand("{if Shots not equal 5}ne{endif}!"), "!");
        s.set_var("Min", 9);
        assert_eq!(s.expand("{if Shots less Min}lt{endif}"), "lt");
        // nested + unknown tokens are preserved
        assert_eq!(s.expand("{if Shots more 1}x{if Shots more 9}y{else}z{endif}{endif}{ZW_SELECT}"), "xz{ZW_SELECT}");
    }
}
