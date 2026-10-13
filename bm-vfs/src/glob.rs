//! Minimal glob matcher over already-normalised (lowercase, `/`-separated) paths.
//! `*` matches any run of characters except `/`, `**` matches anything (including `/`),
//! `?` matches one non-`/` character.

pub fn matches(pattern: &str, text: &str) -> bool {
    m(pattern.as_bytes(), text.as_bytes())
}

fn m(p: &[u8], t: &[u8]) -> bool {
    match p.first() {
        None => t.is_empty(),
        Some(b'*') => {
            let double = p.get(1) == Some(&b'*');
            let rest = if double { &p[2..] } else { &p[1..] };
            // `**/` may also match zero segments
            let rest_skip_slash = if double && rest.first() == Some(&b'/') { Some(&rest[1..]) } else { None };
            if let Some(r) = rest_skip_slash {
                if m(r, t) {
                    return true;
                }
            }
            let mut i = 0;
            loop {
                if m(rest, &t[i..]) {
                    return true;
                }
                if i >= t.len() || (!double && t[i] == b'/') {
                    return false;
                }
                i += 1;
            }
        }
        Some(b'?') => !t.is_empty() && t[0] != b'/' && m(&p[1..], &t[1..]),
        Some(&c) => !t.is_empty() && t[0] == c && m(&p[1..], &t[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn globs() {
        assert!(matches("scenes/**/*.zip", "scenes/m01/m01_main.zip"));
        assert!(matches("scenes/**/*.zip", "scenes/a.zip"));
        assert!(!matches("scenes/*.zip", "scenes/m01/a.zip"));
        assert!(matches("scenes/m01/m01_main.*", "scenes/m01/m01_main.tex"));
        assert!(matches("a?c", "abc"));
        assert!(!matches("a?c", "a/c"));
        assert!(matches("**", "x/y/z"));
        assert!(matches("shaders/pc/fx/*.fx", "shaders/pc/fx/glass.fx"));
    }
}
