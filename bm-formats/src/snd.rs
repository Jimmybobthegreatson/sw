//! `.SND` - sound-engine object database of a scene (which sounds exist and how they are grouped).
//!
//! Only the outer structure is decoded; the object bodies are exposed as raw byte ranges.
//!
//! ```text
//! 0x00 u32 payload_size     = file_len - 16 (hideout_main: file_len - 480)   (same 4-word header as .WHD)
//! 0x04 u32 file_len
//! 0x08 u32 3, 0x0c u32 4
//! 0x10 NUL padded name of the audio stream file ("pc_Eng.str", 16 bytes)
//! 0x20 ...                  objects. Each object body is followed by (or contains) a 16-byte
//!                           aligned name of the form "audio:<path>#<kind>" where kind is one of
//!                           `wave`, `sound`, `seq`, `set`, `group` (a sound hierarchy:
//!                           group -> set/seq -> sound -> wave); bodies hold floats such as volume
//!                           (0.32 ...), flags and indices into the same list.
//! ```
//!
//! The `wave` names match the entries of the scene's `.WHD` bank (`Action/Wipe_Table/Wipe_Table_01.wav`
//! <-> `audio:action/wipe_table/wipe_table_01#wave`).

use crate::io::{invalid, u32_at, FormatError, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SndKind {
    Wave,
    Sound,
    Seq,
    Set,
    Group,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SndName {
    /// File offset of the `audio:` string (16-byte aligned).
    pub offset: u32,
    /// Full string, e.g. `audio:action/wipe_table/wipe_table_01#wave`.
    pub full: String,
    /// Path between `audio:` and `#`.
    pub path: String,
    pub kind: SndKind,
}

#[derive(Clone, Debug)]
pub struct Snd {
    pub stream_name: String,
    pub names: Vec<SndName>,
    pub raw: Vec<u8>,
}

impl Snd {
    pub fn parse(d: &[u8]) -> Result<Snd> {
        let payload = u32_at(d, 0).ok_or(FormatError::Eof("SND header"))? as usize;
        if u32_at(d, 4) != Some(d.len() as u32) || payload > d.len() || u32_at(d, 8) != Some(3) || u32_at(d, 12) != Some(4) {
            return invalid("SND header does not match");
        }
        let name_end = d[16..32].iter().position(|&b| b == 0).unwrap_or(16);
        let stream_name = String::from_utf8_lossy(&d[16..16 + name_end]).into_owned();
        let mut names = Vec::new();
        let mut p = 32;
        while p + 6 <= d.len() {
            if &d[p..p + 6] == b"audio:" {
                let end = d[p..].iter().position(|&b| b == 0).map(|n| p + n).unwrap_or(d.len());
                let full = String::from_utf8_lossy(&d[p..end]).into_owned();
                let (path, kind) = match full[6..].rsplit_once('#') {
                    Some((pa, k)) => (
                        pa.to_string(),
                        match k {
                            "wave" => SndKind::Wave,
                            "sound" => SndKind::Sound,
                            "seq" => SndKind::Seq,
                            "set" => SndKind::Set,
                            "group" => SndKind::Group,
                            o => SndKind::Other(o.to_string()),
                        },
                    ),
                    None => (full[6..].to_string(), SndKind::Other(String::new())),
                };
                names.push(SndName { offset: p as u32, full, path, kind });
                p = (end + 16) & !15;
            } else {
                p += 16;
            }
        }
        Ok(Snd { stream_name, names, raw: d.to_vec() })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.raw.clone()
    }

    pub fn waves(&self) -> impl Iterator<Item = &SndName> {
        self.names.iter().filter(|n| n.kind == SndKind::Wave)
    }
}
