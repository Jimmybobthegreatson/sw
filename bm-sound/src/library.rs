//! Sound library: level sound banks (`.WHD`/`.WAV` + `pc_eng.str`), on-demand IMA ADPCM decoding with
//! an LRU cache, loose-file mod overrides and lip-sync access.
//!
//! # Mod overrides
//!
//! Every lookup first checks the VFS (so mod directories added with `Vfs::add_mod_dir` win) for loose
//! files, in this order, for a bank with stem `M01_main` and a sound named
//! `Action/Wipe_Table/Wipe_Table_01.wav` at index 12:
//!
//! 1. `Sounds/M01_main/Action/Wipe_Table/Wipe_Table_01.{ogg,flac,wav}`   (by name, one bank)
//! 2. `Sounds/M01_main/id_12.{ogg,flac,wav}`                              (by index, one bank)
//! 3. `Sounds/_global/Action/Wipe_Table/Wipe_Table_01.{ogg,flac,wav}`     (by name, all banks)
//!
//! The original extension of the name is ignored. Names that do not exist in any bank can be added the
//! same way and played with [`SoundLibrary::pcm_by_name`].

use crate::decode::{decode_asset, DecodeError};
use crate::lip::{LipSync, LIP_BLOCK_SIZE};
use crate::pcm::Pcm;
use crate::snd_db::SndDb;
use bm_vfs::sound::{decode_entry, SoundBank, SoundEntry};
use bm_vfs::stream::StrIndex;
use bm_vfs::{Vfs, VfsError};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Debug, thiserror::Error)]
pub enum SoundError {
    #[error("vfs: {0}")]
    Vfs(#[from] VfsError),
    #[error("decode: {0}")]
    Decode(#[from] DecodeError),
    #[error("sound not found: {0}")]
    NotFound(String),
    #[error("unsupported sound format of {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, SoundError>;

/// Identifies a sound of a loaded bank.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SoundId {
    pub bank: u16,
    pub index: u32,
}

/// Static description of a bank entry (no decoding needed).
#[derive(Clone, Debug)]
pub struct SoundInfo {
    pub id: SoundId,
    pub name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: u32,
    pub duration: f32,
    /// Data lives in `pc_eng.str` (dialogue / music).
    pub streamed: bool,
    /// Flag bit `0x80` ("PHO") of the record. Set on *every* streamed entry (dialogue with `LIP ` chunks and
    /// music alike); use [`SoundLibrary::lip_sync`] to find out whether lip data exists.
    pub lip_sync: bool,
    /// Raw flag word of the `.WHD` record (`0x80` PHO, `0x100` stream, `0x180`/`0x8280` music...).
    pub flags: u16,
    pub bytes_stored: u32,
}

struct BankSlot {
    base: String,
    stem: String,
    bank: SoundBank,
    snd: Option<SndDb>,
    /// lower-case name -> index
    names: HashMap<String, u32>,
}

#[derive(Hash, PartialEq, Eq, Clone)]
enum Key {
    Id(SoundId),
    Name(String),
}

struct Cache {
    map: HashMap<Key, (Arc<Pcm>, u64)>,
    bytes: usize,
    budget: usize,
    tick: u64,
    hits: u64,
    misses: u64,
}

/// Cache counters.
#[derive(Clone, Copy, Debug, Default)]
pub struct CacheStats {
    pub entries: usize,
    pub bytes: usize,
    pub hits: u64,
    pub misses: u64,
}

pub struct SoundLibrary {
    vfs: Arc<Vfs>,
    banks: Vec<BankSlot>,
    stream: OnceLock<std::result::Result<Arc<StrIndex>, String>>,
    cache: Mutex<Cache>,
}

const EXTS: [&str; 3] = ["ogg", "flac", "wav"];

/// Whether the entry's data lives in `pc_eng.str`. `SoundEntry::is_streamed` misses PCM music entries
/// (`flags` `0x8280`, offsets far beyond the bank's `.WAV`), so flag bit `0x8000` counts as well.
fn is_streamed(e: &SoundEntry) -> bool {
    e.is_streamed() || e.flags() & 0x8000 != 0
}

fn strip_ext(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > name.rfind('/').map_or(0, |s| s + 1) => &name[..i],
        _ => name,
    }
}

impl SoundLibrary {
    /// An empty library (add banks with [`SoundLibrary::load_bank`]).
    pub fn new(vfs: Arc<Vfs>) -> SoundLibrary {
        SoundLibrary {
            vfs,
            banks: Vec::new(),
            stream: OnceLock::new(),
            cache: Mutex::new(Cache { map: HashMap::new(), bytes: 0, budget: 256 << 20, tick: 0, hits: 0, misses: 0 }),
        }
    }

    /// Opens the sound bank (and `.SND`) of one scene, e.g. `Scenes/M01/M01_main`.
    pub fn open_level(vfs: Arc<Vfs>, base: &str) -> Result<SoundLibrary> {
        let mut l = SoundLibrary::new(vfs);
        l.load_bank(base)?;
        Ok(l)
    }

    /// Loads another bank; returns its bank index (`SoundId::bank`). The `.SND` is optional.
    pub fn load_bank(&mut self, base: &str) -> Result<u16> {
        let bank = SoundBank::open(&self.vfs, base)?;
        let snd = self.vfs.open(&format!("{base}.SND")).ok().map(|d| SndDb::parse(&d));
        let names = bank.entries.iter().enumerate().map(|(i, e)| (e.name.to_ascii_lowercase(), i as u32)).collect();
        let stem = base.rsplit(['/', '\\']).next().unwrap_or(base).to_string();
        self.banks.push(BankSlot { base: base.to_string(), stem, bank, snd, names });
        Ok((self.banks.len() - 1) as u16)
    }

    pub fn vfs(&self) -> &Arc<Vfs> {
        &self.vfs
    }

    pub fn bank_count(&self) -> usize {
        self.banks.len()
    }

    pub fn bank_base(&self, bank: u16) -> Option<&str> {
        self.banks.get(bank as usize).map(|b| b.base.as_str())
    }

    /// The decoded sound-object database of a bank.
    pub fn snd(&self, bank: u16) -> Option<&SndDb> {
        self.banks.get(bank as usize)?.snd.as_ref()
    }

    /// Raw bank (directory, raw bytes) for tools.
    pub fn bank(&self, bank: u16) -> Option<&SoundBank> {
        self.banks.get(bank as usize).map(|b| &b.bank)
    }

    /// Cache budget in bytes of decoded PCM (default 256 MiB).
    pub fn set_cache_budget(&self, bytes: usize) {
        let mut c = self.cache.lock().unwrap();
        c.budget = bytes;
        Self::evict(&mut c);
    }

    pub fn cache_stats(&self) -> CacheStats {
        let c = self.cache.lock().unwrap();
        CacheStats { entries: c.map.len(), bytes: c.bytes, hits: c.hits, misses: c.misses }
    }

    pub fn clear_cache(&self) {
        let mut c = self.cache.lock().unwrap();
        c.map.clear();
        c.bytes = 0;
    }

    fn entry(&self, id: SoundId) -> Option<&SoundEntry> {
        self.banks.get(id.bank as usize)?.bank.entries.get(id.index as usize)
    }

    /// Number of entries over all banks.
    pub fn len(&self) -> usize {
        self.banks.iter().map(|b| b.bank.entries.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// First sound called `name` (case-insensitive; `/` or `\`).
    pub fn find(&self, name: &str) -> Option<SoundId> {
        let n = name.replace('\\', "/").to_ascii_lowercase();
        self.banks.iter().enumerate().find_map(|(b, s)| s.names.get(&n).map(|&i| SoundId { bank: b as u16, index: i }))
    }

    pub fn info(&self, id: SoundId) -> Option<SoundInfo> {
        let e = self.entry(id)?;
        // `sample_count` counts samples over all channels (stereo music: twice the frame count).
        let frames = e.sample_count() / e.channels().max(1);
        Some(SoundInfo {
            id,
            name: e.name.clone(),
            sample_rate: e.sample_rate(),
            channels: e.channels().max(1) as u16,
            frames,
            duration: frames as f32 / e.sample_rate().max(1) as f32,
            streamed: is_streamed(e),
            lip_sync: e.flags() & 0x80 != 0,
            flags: e.flags(),
            bytes_stored: e.data_size(),
        })
    }

    /// All sounds of all banks.
    pub fn iter(&self) -> impl Iterator<Item = SoundInfo> + '_ {
        self.banks.iter().enumerate().flat_map(move |(b, s)| {
            (0..s.bank.entries.len()).filter_map(move |i| self.info(SoundId { bank: b as u16, index: i as u32 }))
        })
    }

    /// Names matching a case-insensitive substring.
    pub fn search(&self, needle: &str) -> Vec<SoundInfo> {
        let n = needle.to_ascii_lowercase();
        self.iter().filter(|i| i.name.to_ascii_lowercase().contains(&n)).collect()
    }

    /// Loose-file override path (VFS path) for a bank sound, if a mod provides one.
    pub fn override_path(&self, id: SoundId) -> Option<String> {
        let slot = self.banks.get(id.bank as usize)?;
        let e = slot.bank.entries.get(id.index as usize)?;
        let name = strip_ext(&e.name);
        let cands = [
            format!("Sounds/{}/{}", slot.stem, name),
            format!("Sounds/{}/id_{}", slot.stem, id.index),
            format!("Sounds/_global/{name}"),
        ];
        for base in &cands {
            for ext in EXTS {
                let p = format!("{base}.{ext}");
                if self.vfs.stat(&p).is_some() {
                    return Some(p);
                }
            }
        }
        None
    }

    fn stream(&self) -> Result<Arc<StrIndex>> {
        self.stream
            .get_or_init(|| StrIndex::open(&self.vfs, "pc_eng.str").map(Arc::new).map_err(|e| e.to_string()))
            .clone()
            .map_err(|e| SoundError::Vfs(VfsError::Format(format!("pc_eng.str: {e}"))))
    }

    fn evict(c: &mut Cache) {
        while c.bytes > c.budget && c.map.len() > 1 {
            let oldest = c.map.iter().min_by_key(|(_, (_, t))| *t).map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                if let Some((p, _)) = c.map.remove(&k) {
                    c.bytes -= p.bytes();
                }
            }
        }
    }

    fn cached(&self, key: Key, make: impl FnOnce() -> Result<Pcm>) -> Result<Arc<Pcm>> {
        {
            let mut c = self.cache.lock().unwrap();
            c.tick += 1;
            let t = c.tick;
            if let Some((p, last)) = c.map.get_mut(&key) {
                *last = t;
                let p = p.clone();
                c.hits += 1;
                return Ok(p);
            }
            c.misses += 1;
        }
        let pcm = Arc::new(make()?);
        let mut c = self.cache.lock().unwrap();
        c.tick += 1;
        let t = c.tick;
        c.bytes += pcm.bytes();
        if let Some((old, _)) = c.map.insert(key, (pcm.clone(), t)) {
            c.bytes -= old.bytes();
        }
        Self::evict(&mut c);
        Ok(pcm)
    }

    /// Decoded PCM of a bank sound (mod override applied; cached).
    pub fn pcm(&self, id: SoundId) -> Result<Arc<Pcm>> {
        self.cached(Key::Id(id), || {
            if let Some(p) = self.override_path(id) {
                return self.load_loose(&p);
            }
            let slot = self.banks.get(id.bank as usize).ok_or_else(|| SoundError::NotFound(format!("bank {}", id.bank)))?;
            let e = slot
                .bank
                .entries
                .get(id.index as usize)
                .ok_or_else(|| SoundError::NotFound(format!("sound {}:{}", id.bank, id.index)))?;
            let samples = if is_streamed(e) {
                let st = self.stream()?;
                let mut off = e.data_offset();
                if st.read_at(off, 4)? == b"LIP " {
                    off += LIP_BLOCK_SIZE as u64;
                }
                let data = st.read_at(off, e.data_size() as usize)?;
                decode_entry(e, &data)
            } else {
                slot.bank.decode_pcm16(id.index as usize)
            };
            let samples = samples.ok_or_else(|| SoundError::Unsupported(e.name.clone()))?;
            Ok(Pcm::new(e.channels().max(1) as u16, e.sample_rate(), samples))
        })
    }

    fn load_loose(&self, vfs_path: &str) -> Result<Pcm> {
        let bytes = self.vfs.open(vfs_path)?;
        let ext = vfs_path.rsplit('.').next();
        Ok(decode_asset(bytes, ext)?)
    }

    /// PCM by sound name: bank entries first, then mod-only loose files (`Sounds/_global/<name>` or
    /// `Sounds/<bank stem>/<name>`).
    pub fn pcm_by_name(&self, name: &str) -> Result<Arc<Pcm>> {
        if let Some(id) = self.find(name) {
            return self.pcm(id);
        }
        let n = name.replace('\\', "/");
        let base = strip_ext(&n).to_string();
        let mut cands = vec![format!("Sounds/_global/{base}")];
        cands.extend(self.banks.iter().map(|b| format!("Sounds/{}/{}", b.stem, base)));
        for c in cands {
            for ext in EXTS {
                let p = format!("{c}.{ext}");
                if self.vfs.stat(&p).is_some() {
                    return self.cached(Key::Name(p.to_ascii_lowercase()), || self.load_loose(&p));
                }
            }
        }
        Err(SoundError::NotFound(name.to_string()))
    }

    /// Mod-provided loose sound files below `Sounds/` (VFS paths).
    pub fn list_mod_sounds(&self) -> Vec<String> {
        let mut v = Vec::new();
        for ext in EXTS {
            v.extend(self.vfs.list(&format!("Sounds/**/*.{ext}")).into_iter().map(|e| e.path));
        }
        v.sort();
        v
    }

    /// Lip-sync data of a streamed dialogue line (`None` when the line has none).
    pub fn lip_sync(&self, id: SoundId) -> Result<Option<LipSync>> {
        let Some(e) = self.entry(id) else { return Err(SoundError::NotFound(format!("{id:?}"))) };
        if !is_streamed(e) {
            return Ok(None);
        }
        let st = self.stream()?;
        let head = st.read_at(e.data_offset(), 4)?;
        if head != b"LIP " {
            return Ok(None);
        }
        let blk = st.read_at(e.data_offset(), LIP_BLOCK_SIZE)?;
        Ok(LipSync::parse(&blk))
    }

    /// Raw `LIP ` block (0x1000 bytes) of a streamed entry for external tools.
    pub fn lip_raw(&self, id: SoundId) -> Result<Option<Vec<u8>>> {
        let Some(e) = self.entry(id) else { return Err(SoundError::NotFound(format!("{id:?}"))) };
        if !is_streamed(e) {
            return Ok(None);
        }
        let st = self.stream()?;
        let blk = st.read_at(e.data_offset(), LIP_BLOCK_SIZE)?;
        Ok((&blk[..4] == b"LIP ").then_some(blk))
    }
}
