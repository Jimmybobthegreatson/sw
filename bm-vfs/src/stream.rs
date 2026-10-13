//! `pc_eng.str` - the big (1.8 GB) voice/music stream.
//!
//! What is verified (confidence in brackets):
//!
//! * header: `"IOISNDSTREAM"` (12 bytes), `u32 version = 9`, `u32 hash/id`, `u32 count = 0xd56`,
//!   `u32 first_chunk_offset = 0x100`, then zero padding up to 0x100 [high]
//! * the stream is a sequence of chunks starting at 0x100. Dialogue chunks begin with the tag
//!   `"LIP "` followed by `u32 n, u32 flags(13), ...` (lip-sync / phoneme data, occupies up to the
//!   next 4 KiB), then ADPCM audio. `LIP ` tags always sit on 0x100-aligned offsets (2717 found in
//!   the shipped file; the header count of 3414 also covers chunks without lip data) [medium]
//! * sound-bank entries flagged as *streamed* (`SoundEntry::is_streamed`) store their data offset
//!   (`record[8]`) and byte length (`record[6]`) relative to this file. Offsets either point at a
//!   `"LIP "` chunk or directly at an IMA ADPCM block (block header = `i16 predictor, u8 index,
//!   u8 0`); the run ends at the next `"LIP "` chunk [medium]
//! * `.LOC` leaf nodes of voiced lines carry two identical `u32`s which look like positions
//!   related to this file; their exact unit is **unknown**.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::{Result, Vfs, VfsError};

pub const STR_MAGIC: &[u8; 12] = b"IOISNDSTREAM";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrHeader {
    pub version: u32,
    pub hash: u32,
    /// Chunk count stored in the header (3414 in the shipped file).
    pub count: u32,
    /// Offset of the first chunk (0x100).
    pub first_chunk: u32,
}

/// A `"LIP "` chunk found while scanning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrEntry {
    pub offset: u64,
    /// `u32` right after the tag (meaning of the value unknown; 0x336 for the first chunk).
    pub lip_len_field: u32,
    /// Offset of the next chunk or end of file; audio of this chunk lies in `[offset + 0x1000, next)`.
    pub next: u64,
}

pub struct StrIndex {
    path: PathBuf,
    file: Mutex<File>,
    pub header: StrHeader,
    pub len: u64,
}

impl StrIndex {
    /// Opens the stream through the VFS (it has to be a loose file, never inside a ZIP).
    pub fn open(vfs: &Vfs, name: &str) -> Result<StrIndex> {
        let p = vfs.disk_path(name).ok_or_else(|| VfsError::NotFound(name.to_string()))?;
        StrIndex::open_path(p)
    }

    pub fn open_path(path: impl AsRef<Path>) -> Result<StrIndex> {
        let path = path.as_ref().to_path_buf();
        let mut file = File::open(&path)?;
        let len = file.metadata()?.len();
        let mut h = [0u8; 32];
        file.read_exact(&mut h)?;
        if &h[..12] != STR_MAGIC {
            return Err(VfsError::Format("not an IOISNDSTREAM file".into()));
        }
        let w = |o: usize| u32::from_le_bytes(h[o..o + 4].try_into().unwrap());
        let header = StrHeader { version: w(12), hash: w(16), count: w(20), first_chunk: w(24) };
        Ok(StrIndex { path, file: Mutex::new(file), header, len })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads `len` bytes at `offset` (clamped to the file end).
    pub fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>> {
        if offset >= self.len {
            return Err(VfsError::Format("offset beyond end of stream".into()));
        }
        let len = len.min((self.len - offset) as usize);
        let mut f = self.file.lock().unwrap_or_else(|p| p.into_inner());
        f.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; len];
        f.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Scans `[start, end)` for `"LIP "` chunks (0x100-aligned). `end = None` scans the whole file
    /// (about 1.8 GB of sequential reads).
    pub fn scan_lip_chunks(&self, start: u64, end: Option<u64>) -> Result<Vec<StrEntry>> {
        const CH: usize = 1 << 24;
        let end = end.unwrap_or(self.len).min(self.len);
        let mut f = self.file.lock().unwrap_or_else(|p| p.into_inner());
        let mut pos = start & !0xff;
        let mut out: Vec<StrEntry> = Vec::new();
        let mut buf = vec![0u8; CH];
        while pos < end {
            f.seek(SeekFrom::Start(pos))?;
            let want = CH.min((end - pos) as usize);
            let got = read_up_to(&mut *f, &mut buf[..want])?;
            if got == 0 {
                break;
            }
            let mut o = 0;
            while o + 8 <= got {
                if &buf[o..o + 4] == b"LIP " {
                    let n = u32::from_le_bytes(buf[o + 4..o + 8].try_into().unwrap());
                    out.push(StrEntry { offset: pos + o as u64, lip_len_field: n, next: self.len });
                }
                o += 0x100;
            }
            pos += got as u64;
        }
        for i in 0..out.len().saturating_sub(1) {
            out[i].next = out[i + 1].offset;
        }
        Ok(out)
    }
}

fn read_up_to(f: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        let k = f.read(&mut buf[n..])?;
        if k == 0 {
            break;
        }
        n += k;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{game_dir_if_present, Vfs};

    #[test]
    fn real_stream_header() {
        let Some(dir) = game_dir_if_present() else {
            eprintln!("BM_GAME_DIR not available, skipping");
            return;
        };
        let vfs = Vfs::new(dir).unwrap();
        let s = StrIndex::open(&vfs, "pc_eng.str").unwrap();
        assert_eq!(s.header.version, 9);
        assert_eq!(s.header.first_chunk, 0x100);
        let lips = s.scan_lip_chunks(0, Some(4 << 20)).unwrap();
        assert_eq!(lips[0].offset, 0x100);
        assert_eq!(lips[0].lip_len_field, 0x336);
        assert!(lips.len() > 3);
        assert_eq!(lips[0].next, lips[1].offset);
    }
}
