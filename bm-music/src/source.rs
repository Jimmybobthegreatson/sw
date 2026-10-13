//! Random-access audio sources and the resolvers that turn a [`TrackDef`] into one.

use crate::def::{SourceRef, TrackDef};
use crate::error::{MusicError, Result};
use crate::modding;
use bm_sound::{Pcm, SoundLibrary};
use bm_vfs::{StrIndex, Vfs};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Seekable audio. Reads happen on the prefetch thread (or the caller's thread in manual mode), never
/// on the audio callback.
pub trait AudioSource: Send {
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> u16;
    fn len_frames(&self) -> u64;
    /// Reads interleaved frames starting at frame `start`; `out.len()` is a multiple of `channels()`.
    /// Returns the number of frames written (fewer at the end of the source).
    fn read(&mut self, start: u64, out: &mut [f32]) -> usize;
    /// True when this is a loose mod file standing in for the original audio (the original `trim` is
    /// then not applied).
    fn replaced(&self) -> bool {
        false
    }
}

/// In-memory audio (decoded loose files, small bank sounds, synthetic test signals).
#[derive(Clone)]
pub struct MemSource {
    rate: u32,
    channels: u16,
    data: MemData,
    replacement: bool,
}

#[derive(Clone)]
enum MemData {
    I16(Arc<[i16]>),
    F32(Arc<[f32]>),
}

impl MemSource {
    pub fn from_pcm(p: &Pcm) -> MemSource {
        MemSource { rate: p.sample_rate, channels: p.channels, data: MemData::I16(p.samples.clone()), replacement: false }
    }
    /// Marks this source as a mod replacement of original audio (see [`AudioSource::replaced`]).
    pub fn with_replacement(mut self, yes: bool) -> MemSource {
        self.replacement = yes;
        self
    }
    pub fn from_f32(rate: u32, channels: u16, samples: Vec<f32>) -> MemSource {
        MemSource { rate, channels: channels.max(1), data: MemData::F32(samples.into()), replacement: false }
    }
}

impl AudioSource for MemSource {
    fn replaced(&self) -> bool {
        self.replacement
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn len_frames(&self) -> u64 {
        let n = match &self.data {
            MemData::I16(d) => d.len(),
            MemData::F32(d) => d.len(),
        };
        (n / self.channels as usize) as u64
    }
    fn read(&mut self, start: u64, out: &mut [f32]) -> usize {
        let ch = self.channels as usize;
        let total = self.len_frames();
        if start >= total {
            return 0;
        }
        let n = ((out.len() / ch) as u64).min(total - start) as usize;
        let s = start as usize * ch;
        match &self.data {
            MemData::I16(d) => {
                for (o, &v) in out[..n * ch].iter_mut().zip(&d[s..s + n * ch]) {
                    *o = v as f32 * (1.0 / 32768.0);
                }
            }
            MemData::F32(d) => out[..n * ch].copy_from_slice(&d[s..s + n * ch]),
        }
        n
    }
}

/// 16-bit PCM read straight from `pc_eng.str` on demand (the original streamed music).
pub struct StrPcmSource {
    stream: Arc<StrIndex>,
    base: u64,
    rate: u32,
    channels: u16,
    frames: u64,
    scratch: Vec<u8>,
}

impl StrPcmSource {
    pub fn new(stream: Arc<StrIndex>, offset: u64, bytes: u64, rate: u32, channels: u16) -> Result<StrPcmSource> {
        let mut base = offset;
        if stream.read_at(base, 4)? == b"LIP " {
            base += 0x1000;
        }
        let ch = channels.max(1);
        let usable = bytes.saturating_sub(base - offset);
        Ok(StrPcmSource { stream, base, rate, channels: ch, frames: usable / (2 * ch as u64), scratch: Vec::new() })
    }
}

impl AudioSource for StrPcmSource {
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn len_frames(&self) -> u64 {
        self.frames
    }
    fn read(&mut self, start: u64, out: &mut [f32]) -> usize {
        let ch = self.channels as usize;
        if start >= self.frames {
            return 0;
        }
        let n = ((out.len() / ch) as u64).min(self.frames - start) as usize;
        match self.stream.read_at(self.base + start * ch as u64 * 2, n * ch * 2) {
            Ok(bytes) => {
                let got = bytes.len() / (2 * ch);
                self.scratch.clear();
                for (o, b) in out.iter_mut().zip(bytes.chunks_exact(2)).take(got * ch) {
                    *o = i16::from_le_bytes([b[0], b[1]]) as f32 * (1.0 / 32768.0);
                }
                got
            }
            Err(e) => {
                log::warn!("music stream read failed: {e}");
                0
            }
        }
    }
}

/// Context handed to a resolver (used for loose-file override lookup).
#[derive(Clone, Debug)]
pub struct OpenContext {
    pub cue: String,
    pub stem: String,
    /// First stem of the cue (also answers to `Music/<level>/<cue>.<ext>`).
    pub primary: bool,
    /// Key of the track in `MusicDef::tracks`.
    pub track_key: String,
}

pub trait TrackResolver: Send + Sync {
    fn open(&self, ctx: &OpenContext, track: &TrackDef) -> Result<Box<dyn AudioSource>>;
}

/// Resolver backed by named in-memory sources (tests, procedural music). `SourceRef::Bank(name)` and
/// `SourceRef::File(name)` look the name up.
#[derive(Default)]
pub struct MemoryResolver {
    items: Mutex<HashMap<String, MemSource>>,
}

impl MemoryResolver {
    pub fn new() -> MemoryResolver {
        MemoryResolver::default()
    }
    pub fn insert(&self, name: &str, src: MemSource) {
        self.items.lock().unwrap().insert(name.to_string(), src);
    }
}

impl TrackResolver for MemoryResolver {
    fn open(&self, _ctx: &OpenContext, track: &TrackDef) -> Result<Box<dyn AudioSource>> {
        let name = match &track.source {
            SourceRef::Bank(n) | SourceRef::File(n) => n.clone(),
            SourceRef::Slot(s) => format!("slot{s}"),
        };
        self.items
            .lock()
            .unwrap()
            .get(&name)
            .cloned()
            .map(|s| Box::new(s) as Box<dyn AudioSource>)
            .ok_or_else(|| MusicError::Track(name, "not registered".into()))
    }
}

/// Resolver for the real game: bank sounds (streamed from `pc_eng.str`), `.SND` slot table, and loose
/// mod files.
pub struct GameResolver {
    vfs: Arc<Vfs>,
    lib: Arc<SoundLibrary>,
    stream: Option<Arc<StrIndex>>,
    level: String,
    /// Slot -> bank sound name (from the level's music table).
    slots: Vec<Option<String>>,
    loose: Mutex<HashMap<String, Arc<Pcm>>>,
}

impl GameResolver {
    pub fn new(vfs: Arc<Vfs>, lib: Arc<SoundLibrary>, level: &str, slots: Vec<Option<String>>) -> GameResolver {
        let stream = StrIndex::open(&vfs, "pc_eng.str").ok().map(Arc::new);
        GameResolver { vfs, lib, stream, level: level.to_string(), slots, loose: Mutex::new(HashMap::new()) }
    }

    pub fn level(&self) -> &str {
        &self.level
    }

    fn load_loose(&self, path: &str, replacement: bool) -> Result<Box<dyn AudioSource>> {
        let key = path.to_ascii_lowercase();
        if let Some(p) = self.loose.lock().unwrap().get(&key) {
            return Ok(Box::new(MemSource::from_pcm(p).with_replacement(replacement)));
        }
        let bytes = self.vfs.open(path)?;
        let pcm = Arc::new(bm_sound::decode::decode_asset(bytes, path.rsplit('.').next())?);
        let mut m = self.loose.lock().unwrap();
        if m.len() >= 6 {
            m.clear();
        }
        m.insert(key, pcm.clone());
        Ok(Box::new(MemSource::from_pcm(&pcm).with_replacement(replacement)))
    }

    fn open_bank(&self, name: &str) -> Result<Box<dyn AudioSource>> {
        let id = self.lib.find(name).ok_or_else(|| MusicError::Track(name.to_string(), "not in the loaded sound banks".into()))?;
        if let Some(p) = self.lib.override_path(id) {
            return self.load_loose(&p, true);
        }
        if let (Some(bank), Some(stream)) = (self.lib.bank(id.bank), &self.stream) {
            if let Some(e) = bank.entries.get(id.index as usize) {
                let pcm16 = e.format_tag() == 1 && e.bits_per_sample() == 16;
                if pcm16 && (e.is_streamed() || e.flags() & 0x8000 != 0) {
                    let src = StrPcmSource::new(stream.clone(), e.data_offset(), e.data_size() as u64, e.sample_rate(), e.channels() as u16)?;
                    return Ok(Box::new(src));
                }
            }
        }
        let pcm = self.lib.pcm(id)?;
        Ok(Box::new(MemSource::from_pcm(&pcm)))
    }
}

impl TrackResolver for GameResolver {
    fn open(&self, ctx: &OpenContext, track: &TrackDef) -> Result<Box<dyn AudioSource>> {
        if let Some(p) = modding::find_override(&self.vfs, &self.level, &ctx.cue, &ctx.stem, ctx.primary, &ctx.track_key) {
            return self.load_loose(&p, true);
        }
        match &track.source {
            SourceRef::File(p) => self.load_loose(p, false),
            SourceRef::Bank(n) => self.open_bank(n),
            SourceRef::Slot(s) => {
                let name = self
                    .slots
                    .get(*s as usize)
                    .and_then(|x| x.clone())
                    .ok_or_else(|| MusicError::Track(format!("slot {s}"), "empty music slot".into()))?;
                self.open_bank(&name)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_source_reads_and_clamps() {
        let mut s = MemSource::from_f32(1000, 2, (0..20).map(|i| i as f32).collect());
        assert_eq!(s.len_frames(), 10);
        let mut b = [0.0f32; 8];
        assert_eq!(s.read(3, &mut b), 4);
        assert_eq!(b[0], 6.0);
        assert_eq!(s.read(8, &mut b), 2);
        assert_eq!(s.read(10, &mut b), 0);
    }
}
