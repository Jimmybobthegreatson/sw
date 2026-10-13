//! `.BUF` - name/data buffer that accompanies a `.GMS`.
//!
//! Layout (confidence: medium - only the name pool is relied upon):
//!
//! * `0x00`: the 16 ASCII bytes `"0123456789abcdef"`, followed by what look like lookup tables
//!   (`u16` pairs, character-class tables `-./0123456789:`, `A-Z`, `a-z` ...). **Unknown purpose.**
//! * somewhere after those tables: the geometry name pool - NUL terminated strings in 16 byte
//!   aligned slots (slot length varies: 0x20, 0x30 ...). [`crate::gms::GeomHeader::name_offset`]
//!   points straight at the first character of a name.
//!
//! The file is kept as raw bytes (it round-trips trivially); only name lookup is provided.

use crate::io::{latin1, FormatError, Result};

#[derive(Clone, Debug)]
pub struct Buf {
    pub data: Vec<u8>,
}

impl Buf {
    pub fn parse(data: &[u8]) -> Result<Buf> {
        if data.len() < 16 || &data[..16] != b"0123456789abcdef" {
            return Err(FormatError::Invalid("BUF does not start with the hex digit table".into()));
        }
        Ok(Buf { data: data.to_vec() })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.data.clone()
    }

    /// NUL-terminated string starting at `offset` (Latin-1).
    pub fn name_at(&self, offset: u32) -> Option<String> {
        let tail = self.data.get(offset as usize..)?;
        let n = tail.iter().position(|&b| b == 0)?;
        Some(latin1(&tail[..n]))
    }
}
