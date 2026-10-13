//! Tolerant XML subset parser for the Glacier menu files (`menu/*.xml`).
//!
//! The shipped files are *not* well-formed XML: they contain duplicate attributes
//! (`<Template Name="A" Name="B">`), raw `<`/`>` inside attribute values (`Text="<fix to dynamic name>"`),
//! and are Latin-1 or UTF-8. A strict parser rejects 3 of the 12 distinct files, so this one keeps the first
//! occurrence of a duplicate attribute, records a warning and carries on.
//!
//! Only elements and attributes are modelled (the menu format carries no text content).

use std::fmt;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
    /// 1-based line of the opening tag.
    pub line: u32,
}

impl Element {
    /// Attribute lookup, ASCII case-insensitive (the engine's parser is).
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn has_attr(&self, name: &str) -> bool {
        self.attr(name).is_some()
    }

    /// `True`/`False`/`1`/`0` (case-insensitive) as a bool.
    pub fn attr_bool(&self, name: &str) -> Option<bool> {
        parse_bool(self.attr(name)?)
    }

    pub fn attr_f32(&self, name: &str) -> Option<f32> {
        self.attr(name)?.trim().parse().ok()
    }

    pub fn is(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.is(name))
    }
}

pub fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct XmlWarning {
    pub line: u32,
    pub message: String,
}

impl fmt::Display for XmlWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

#[derive(Clone, Debug)]
pub struct XmlDoc {
    /// Synthetic root holding every top-level element (there is normally exactly one).
    pub root: Element,
    pub warnings: Vec<XmlWarning>,
}

/// Decodes bytes as UTF-8, falling back to Latin-1. Strips a BOM.
pub fn decode_text(bytes: &[u8]) -> String {
    let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| c as char).collect(),
    }
}

pub fn parse_bytes(bytes: &[u8]) -> XmlDoc {
    parse(&decode_text(bytes))
}

pub fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        let Some(semi) = rest.find(';').filter(|&i| i <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..semi];
        let rep = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32)
            }
            _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match rep {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn parse(src: &str) -> XmlDoc {
    let b = src.as_bytes();
    let mut warnings = Vec::new();
    let mut i = 0usize;
    let mut line = 1u32;
    // stack of open elements; index 0 is the synthetic root
    let mut stack: Vec<Element> = vec![Element { name: "#root".into(), ..Default::default() }];

    macro_rules! advance {
        ($n:expr) => {{
            for &c in &b[i..($n).min(b.len())] {
                if c == b'\n' {
                    line += 1;
                }
            }
            i = ($n).min(b.len());
        }};
    }

    while i < b.len() {
        if b[i] != b'<' {
            let next = src[i..].find('<').map(|p| i + p).unwrap_or(b.len());
            advance!(next);
            continue;
        }
        if src[i..].starts_with("<!--") {
            let end = src[i + 4..].find("-->").map(|p| i + 4 + p + 3).unwrap_or(b.len());
            advance!(end);
            continue;
        }
        if src[i..].starts_with("<?") || src[i..].starts_with("<!") {
            let end = src[i..].find('>').map(|p| i + p + 1).unwrap_or(b.len());
            advance!(end);
            continue;
        }
        if src[i..].starts_with("</") {
            let end = src[i..].find('>').map(|p| i + p).unwrap_or(b.len());
            let name = src[i + 2..end].trim().to_string();
            advance!(end + 1);
            // pop to the matching open element
            if let Some(pos) = stack.iter().rposition(|e| e.name.eq_ignore_ascii_case(&name)).filter(|&p| p > 0) {
                while stack.len() > pos + 1 {
                    let e = stack.pop().unwrap();
                    warnings.push(XmlWarning { line, message: format!("<{}> closed implicitly by </{name}>", e.name) });
                    stack.last_mut().unwrap().children.push(e);
                }
                let e = stack.pop().unwrap();
                stack.last_mut().unwrap().children.push(e);
            } else {
                warnings.push(XmlWarning { line, message: format!("stray </{name}> ignored") });
            }
            continue;
        }
        // start tag
        let tag_line = line;
        let mut j = i + 1;
        let name_start = j;
        while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'>' && b[j] != b'/' {
            j += 1;
        }
        let name = src[name_start..j].to_string();
        let mut el = Element { name, line: tag_line, ..Default::default() };
        let mut self_closing = false;
        loop {
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j >= b.len() {
                break;
            }
            match b[j] {
                b'>' => {
                    j += 1;
                    break;
                }
                b'/' => {
                    self_closing = true;
                    j += 1;
                }
                _ => {
                    let ks = j;
                    while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'=' && b[j] != b'>' && b[j] != b'/' {
                        j += 1;
                    }
                    let key = src[ks..j].to_string();
                    while j < b.len() && b[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    let mut val = String::new();
                    if j < b.len() && b[j] == b'=' {
                        j += 1;
                        while j < b.len() && b[j].is_ascii_whitespace() {
                            j += 1;
                        }
                        if j < b.len() && (b[j] == b'"' || b[j] == b'\'') {
                            let q = b[j];
                            let vs = j + 1;
                            let ve = b[vs..].iter().position(|&c| c == q).map(|p| vs + p).unwrap_or(b.len());
                            val = unescape(&src[vs..ve]);
                            j = (ve + 1).min(b.len());
                        } else {
                            let vs = j;
                            while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'>' && !(b[j] == b'/' && b.get(j + 1) == Some(&b'>')) {
                                j += 1;
                            }
                            val = unescape(&src[vs..j]);
                        }
                    }
                    if key.is_empty() {
                        j += 1; // make progress on garbage
                    } else if el.has_attr(&key) {
                        warnings.push(XmlWarning {
                            line: tag_line,
                            message: format!("duplicate attribute {key} on <{}>: keeping the first", el.name),
                        });
                    } else {
                        el.attrs.push((key, val));
                    }
                }
            }
        }
        advance!(j);
        if self_closing {
            stack.last_mut().unwrap().children.push(el);
        } else {
            stack.push(el);
        }
    }
    while stack.len() > 1 {
        let e = stack.pop().unwrap();
        warnings.push(XmlWarning { line, message: format!("<{}> (line {}) never closed", e.name, e.line) });
        stack.last_mut().unwrap().children.push(e);
    }
    XmlDoc { root: stack.pop().unwrap(), warnings }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_tree_and_attrs() {
        let d = parse(r#"<A x="1"><!-- c --><B y='2' z=3/><C></C></A>"#);
        let a = &d.root.children[0];
        assert_eq!(a.name, "A");
        assert_eq!(a.attr("X"), Some("1"));
        assert_eq!(a.children.len(), 2);
        assert_eq!(a.children[0].attr("z"), Some("3"));
        assert!(d.warnings.is_empty());
    }

    #[test]
    fn tolerates_duplicate_attrs_and_raw_angle_brackets() {
        let d = parse(r#"<R><T Name="P" Name="Q"/><G Text="<fix here>" ID="m"/></R>"#);
        let r = &d.root.children[0];
        assert_eq!(r.children[0].attr("Name"), Some("P"));
        assert_eq!(r.children[1].attr("Text"), Some("<fix here>"));
        assert_eq!(r.children[1].attr("ID"), Some("m"));
        assert_eq!(d.warnings.len(), 1);
    }

    #[test]
    fn entities_and_unclosed() {
        let d = parse(r#"<A t="a&amp;b &#65; &lt;"><B>"#);
        assert_eq!(d.root.children[0].attr("t"), Some("a&b A <"));
        assert_eq!(d.root.children[0].children[0].name, "B");
        assert!(!d.warnings.is_empty());
    }

    #[test]
    fn latin1_fallback() {
        assert_eq!(decode_text(&[b'a', 0xE9]), "a\u{e9}");
        assert_eq!(decode_text(&[0xEF, 0xBB, 0xBF, b'x']), "x");
    }
}
