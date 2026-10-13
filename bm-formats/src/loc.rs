//! `.LOC` - localisation string tree.
//!
//! # Layout (confidence: high, round-trips byte-exactly on every shipped scene; the leaf trailer
//! semantics are medium/low)
//!
//! ```text
//! u8   top_count                      number of top-level nodes ("AllLevels", "M01", ...)
//! u32  top_pos[top_count - 1]         start of nodes 1.. relative to the end of this table
//! node...
//!
//! node   := key (NUL-terminated UTF-8)  flag:u8  payload
//! flag 0x10 (group):  u8 count; u32 pos[count - 1] (start of children 1.. relative to the end of
//!                     the pos table; child 0 starts right after it); count x node
//! flag 0x09 (text):   text (NUL-terminated UTF-8) + 4 zero bytes
//! flag 0x29 (voiced): text (NUL-terminated UTF-8) + u32 a + u32 b   (a == b in practice; they look
//!                     like positions related to `pc_eng.str`, unit unknown) -- 3 nodes in M01 only
//!                     carry 4 trailer bytes instead of 8
//! flag 0x08 (empty):  no payload
//! ```
//!
//! A leaf's extent is implied by the next sibling's start (or the parent's end), so the parser keeps
//! each leaf's raw payload; `Leaf::text()` cuts it at the first NUL and `Leaf::trailer()` is the rest.
//! Text may contain markup such as `{if shotsfired more 100}`, `<br>`, `<font 2>`, `<color ffffff>`
//! and `{bullet_square}` (interpreted by the UI/news system, not here).

use crate::io::{invalid, u32_at, FormatError, PutLe, Result};

pub const FLAG_GROUP: u8 = 0x10;
pub const FLAG_TEXT: u8 = 0x09;
pub const FLAG_VOICED: u8 = 0x29;
pub const FLAG_EMPTY: u8 = 0x08;

#[derive(Clone, Debug, PartialEq)]
pub struct Leaf {
    pub flag: u8,
    /// Raw payload bytes (text + NUL + trailer).
    pub payload: Vec<u8>,
}

impl Leaf {
    /// The text up to the first NUL (empty for flag 0x08 leaves).
    pub fn text(&self) -> String {
        let end = self.payload.iter().position(|&b| b == 0).unwrap_or(self.payload.len());
        String::from_utf8_lossy(&self.payload[..end]).into_owned()
    }

    /// Bytes after the text's NUL terminator.
    pub fn trailer(&self) -> &[u8] {
        match self.payload.iter().position(|&b| b == 0) {
            Some(i) => &self.payload[i + 1..],
            None => &[],
        }
    }

    /// For voiced lines (flag 0x29): the two trailer words.
    pub fn stream_words(&self) -> Option<(u32, u32)> {
        let t = self.trailer();
        Some((u32_at(t, 0)?, u32_at(t, 4)?))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum LocBody {
    Group(Vec<LocNode>),
    Leaf(Leaf),
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocNode {
    pub key: String,
    pub body: LocBody,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Loc {
    pub top: Vec<LocNode>,
}

impl Loc {
    pub fn parse(d: &[u8]) -> Result<Loc> {
        let n = *d.first().ok_or(FormatError::Eof("LOC header"))? as usize;
        if n == 0 {
            return Ok(Loc { top: Vec::new() });
        }
        let (starts, ends) = child_ranges(d, 1, n, d.len())?;
        let mut top = Vec::with_capacity(n);
        for (s, e) in starts.into_iter().zip(ends) {
            top.push(parse_node(d, s, e, 0)?);
        }
        Ok(Loc { top })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        write_children(&mut out, &self.top, true);
        out
    }

    /// All text leaves as `(path, leaf)`; path segments joined with `/`.
    pub fn leaves(&self) -> Vec<(String, &Leaf)> {
        fn walk<'a>(n: &'a LocNode, prefix: &str, out: &mut Vec<(String, &'a Leaf)>) {
            let path = if prefix.is_empty() { n.key.clone() } else { format!("{prefix}/{}", n.key) };
            match &n.body {
                LocBody::Group(c) => c.iter().for_each(|c| walk(c, &path, out)),
                LocBody::Leaf(l) => out.push((path, l)),
            }
        }
        let mut out = Vec::new();
        for n in &self.top {
            walk(n, "", &mut out);
        }
        out
    }

    /// Looks a text up by `/` separated path (ASCII case-insensitive), e.g. `AllLevels/Actions/ActionKill`.
    pub fn get(&self, path: &str) -> Option<String> {
        let mut level: &[LocNode] = &self.top;
        let mut parts = path.split('/').peekable();
        while let Some(p) = parts.next() {
            let node = level.iter().find(|n| n.key.eq_ignore_ascii_case(p))?;
            match &node.body {
                LocBody::Group(c) => level = c,
                LocBody::Leaf(l) => return if parts.peek().is_none() { Some(l.text()) } else { None },
            }
        }
        None
    }
}

/// `count` children of the table at `table_at`: returns (start, end) offsets.
fn child_ranges(d: &[u8], table_at: usize, count: usize, parent_end: usize) -> Result<(Vec<usize>, Vec<usize>)> {
    let npos = count.saturating_sub(1);
    let base = table_at + npos * 4;
    if base > d.len() {
        return Err(FormatError::Eof("LOC position table"));
    }
    let mut starts = vec![base];
    for i in 0..npos {
        let p = u32_at(d, table_at + i * 4).ok_or(FormatError::Eof("LOC position"))? as usize;
        starts.push(base + p);
    }
    let mut ends: Vec<usize> = starts[1..].to_vec();
    ends.push(parent_end);
    for (s, e) in starts.iter().zip(&ends) {
        if s > e || *e > d.len() {
            return invalid("LOC child range out of order");
        }
    }
    Ok((starts, ends))
}

fn parse_node(d: &[u8], start: usize, end: usize, depth: usize) -> Result<LocNode> {
    if depth > 32 {
        return invalid("LOC nesting too deep");
    }
    let nul = d[start..end].iter().position(|&b| b == 0).ok_or(FormatError::Eof("LOC key"))?;
    let key = String::from_utf8_lossy(&d[start..start + nul]).into_owned();
    let flag = *d.get(start + nul + 1).ok_or(FormatError::Eof("LOC flag"))?;
    let q = start + nul + 2;
    if flag == FLAG_GROUP {
        let count = *d.get(q).ok_or(FormatError::Eof("LOC count"))? as usize;
        let mut children = Vec::with_capacity(count);
        if count > 0 {
            let (starts, ends) = child_ranges(d, q + 1, count, end)?;
            for (s, e) in starts.into_iter().zip(ends) {
                children.push(parse_node(d, s, e, depth + 1)?);
            }
        }
        Ok(LocNode { key, body: LocBody::Group(children) })
    } else {
        Ok(LocNode { key, body: LocBody::Leaf(Leaf { flag, payload: d[q..end].to_vec() }) })
    }
}

/// Writes `children` as `[count][positions][children]` for the top level (`top = true`) or just
/// `[positions][children]` (groups write their count byte themselves).
fn write_children(out: &mut Vec<u8>, children: &[LocNode], top: bool) {
    if top {
        out.put_u8(children.len() as u8);
    }
    let mut bodies: Vec<Vec<u8>> = Vec::with_capacity(children.len());
    for c in children {
        let mut b = Vec::new();
        write_node(&mut b, c);
        bodies.push(b);
    }
    let mut pos = 0u32;
    for (i, b) in bodies.iter().enumerate() {
        if i > 0 {
            out.put_u32(pos);
        }
        pos += b.len() as u32;
        // positions are relative to the end of the table, so keep a running offset
    }
    // (the loop above emitted `pos` *before* adding, which for i > 0 is the start of child i)
    for b in bodies {
        out.extend_from_slice(&b);
    }
}

fn write_node(out: &mut Vec<u8>, n: &LocNode) {
    out.extend_from_slice(n.key.as_bytes());
    out.push(0);
    match &n.body {
        LocBody::Group(c) => {
            out.push(FLAG_GROUP);
            out.put_u8(c.len() as u8);
            write_children(out, c, false);
        }
        LocBody::Leaf(l) => {
            out.push(l.flag);
            out.extend_from_slice(&l.payload);
        }
    }
}
