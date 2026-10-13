//! Deeper decoding of the `.SND` sound-object database (see `docs/sound.md` for the evidence and
//! confidence level of every field).
//!
//! `.SND` is a flat memory image of the engine's sound-object graph. Objects are identified by names
//! `audio:<path>#<kind>`; every pointer in the file is an absolute file offset. The pieces we rely on:
//!
//! * **Body header** (16-byte aligned): `u32 kind, u32 0xcdcdcdcd, u32 name_ptr, u16 n, u16 0xcdcd`
//!   where `name_ptr` points at the object's `audio:` string.
//! * **Wave record** (`kind` 4): `u32 4, u32 strlen, u32 file_name_ptr, 0, 0, u32 whd_record_offset,
//!   f32 duration, 0, u32 1, 0xcdcdcdcd, u32 sound_name_ptr, ...`. (`+4` is the file name length for
//!   resident waves and a constant-looking packed word for streamed ones.)
//! * **Room effect** (`#roomeffect`): an EAX 4 reverb parameter block, see [`RoomEffect`].
//!
//! Scene nodes (`ZSNDOBJ` and the `Audio_*` controllers) refer to objects by the file offset of the
//! body header; [`SndDb::object_at`] resolves such an offset.

use std::collections::HashMap;

pub use bm_formats::snd::SndKind;

fn u32_at(d: &[u8], p: usize) -> Option<u32> {
    d.get(p..p + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
fn i32_at(d: &[u8], p: usize) -> Option<i32> {
    u32_at(d, p).map(|v| v as i32)
}
fn f32_at(d: &[u8], p: usize) -> Option<f32> {
    u32_at(d, p).map(f32::from_bits)
}
fn u16_at(d: &[u8], p: usize) -> Option<u16> {
    d.get(p..p + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

/// One `audio:<path>#<kind>` object.
#[derive(Clone, Debug)]
pub struct SndObject {
    /// File offset of the name string.
    pub name_off: u32,
    /// Lower-case path between `audio:` and `#`.
    pub path: String,
    pub kind: SndKind,
    /// File offset of the object's body header, if one was found.
    pub body_off: Option<u32>,
    /// `u16` at body+0x0c (child / wave count for sounds).
    pub count: u16,
}

/// A `#wave` object: the link between the sound graph and a `.WHD` bank entry.
#[derive(Clone, Debug)]
pub struct WaveObject {
    pub obj: usize,
    /// Original file name (`Action/Wipe_Table/Wipe_Table_01.wav`); matches the bank entry name.
    pub file: String,
    /// Offset of the entry's 52-byte record inside the `.WHD` (= `SoundEntry::whd_offset` + the 16-byte
    /// padded name length). Verified; join by [`WaveObject::file`] is simpler.
    pub whd_offset: u32,
    /// File offset of the owning `#sound` name. Verified (every wave points at a `#sound` name).
    pub sound_name_off: u32,
    /// Duration in seconds (`f32` at +0x18). Verified: equals `sample_count / sample_rate`.
    pub duration: f32,
    /// `f32` at +0x38 (`?`: 1.0 for most waves; looks like a per-wave gain / pitch factor).
    pub x38: f32,
    /// Raw 0x50-byte record for further research.
    pub raw: Vec<u8>,
}

/// EAX 4.0 reverb preset block (`audio:roomeffects/...#roomeffect`). Field order follows
/// `EAXREVERBPROPERTIES`; values verified against the well-known "Generic" preset
/// (-1000, -100, 1.49, 0.83, -2602, 0.007, 200, 0.011, 0.25, -5, 5000, 250, flags 0x3f).
#[derive(Clone, Debug, PartialEq)]
pub struct RoomEffect {
    pub name: String,
    /// Three leading words (`?`, second one is 2 in all observed objects).
    pub head: [u32; 3],
    pub environment_size: f32,
    pub environment_diffusion: f32,
    /// mB (hundredths of dB).
    pub room: i32,
    pub room_hf: i32,
    pub room_lf: i32,
    /// Seconds.
    pub decay_time: f32,
    pub decay_hf_ratio: f32,
    pub decay_lf_ratio: f32,
    pub reflections: i32,
    pub reflections_delay: f32,
    pub reverb: i32,
    pub reverb_delay: f32,
    pub echo_time: f32,
    pub echo_depth: f32,
    pub modulation_time: f32,
    pub modulation_depth: f32,
    pub air_absorption_hf: f32,
    pub hf_reference: f32,
    pub lf_reference: f32,
    pub room_rolloff: f32,
    pub flags: u32,
}

/// `#surfaceproperty` (footstep/impact material).
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceProperty {
    pub name: String,
    /// First 16 words of the body (`?`).
    pub words: Vec<u32>,
}

/// Parameters read from a `#sound` body (offsets relative to the body header; all `M`/`?`, see
/// `docs/sound.md` for the statistics behind them).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundParams {
    /// `f32` at +0x18 (10.0 for a fountain, 35.0 waterfall, 14.0 bomb explosion): probably the
    /// reference/"full volume" distance in metres (`?`).
    pub range_m: f32,
    /// `i16` at +0x1c and +0x22: one of them is -60/-100 (outer cone gain or minimum volume, in
    /// 1/100 dB or percent; `?`), the other 0.
    pub gain_a: i16,
    pub gain_b: i16,
    /// Cone angles in degrees (360 in every observed sound = omnidirectional). `M`.
    pub cone_inner: u16,
    pub cone_outer: u16,
    /// Byte at +0x27: volume in percent (100, 50, 17 ...). `M`.
    pub volume_pct: u8,
    /// Byte at +0x28 (1 or 2; ambient loops use 1, one-shots 2). `?`
    pub mode: u8,
    /// Byte at +0x29 (0x14, 0x1e, 0x23 on dialogue lines, 0 for effects; priority/cooldown?). `?`
    pub aux: u8,
}

impl SoundParams {
    pub fn volume(&self) -> f32 {
        self.volume_pct as f32 / 100.0
    }
}

#[derive(Clone, Debug)]
pub struct SndDb {
    pub objects: Vec<SndObject>,
    pub waves: Vec<WaveObject>,
    pub room_effects: Vec<RoomEffect>,
    pub surfaces: Vec<SurfaceProperty>,
    by_body: HashMap<u32, usize>,
    by_name_off: HashMap<u32, usize>,
    by_path: HashMap<(String, u8), usize>,
    /// sound object index -> wave indices
    sound_waves: HashMap<usize, Vec<usize>>,
    raw: Vec<u8>,
}

fn kind_tag(k: &SndKind) -> u8 {
    match k {
        SndKind::Wave => 0,
        SndKind::Sound => 1,
        SndKind::Seq => 2,
        SndKind::Set => 3,
        SndKind::Group => 4,
        SndKind::Other(_) => 5,
    }
}

impl SndDb {
    pub fn parse(d: &[u8]) -> SndDb {
        let mut objects: Vec<SndObject> = Vec::new();
        let mut by_name_off = HashMap::new();
        let mut p = 32usize;
        while p + 6 <= d.len() {
            if &d[p..p + 6] == b"audio:" {
                let end = d[p..].iter().position(|&b| b == 0).map(|n| p + n).unwrap_or(d.len());
                let full = String::from_utf8_lossy(&d[p + 6..end]).into_owned();
                let (path, kind) = match full.rsplit_once('#') {
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
                    None => (full, SndKind::Other(String::new())),
                };
                by_name_off.insert(p as u32, objects.len());
                objects.push(SndObject { name_off: p as u32, path, kind, body_off: None, count: 0 });
                p = (end + 16) & !15;
            } else {
                p += 16;
            }
        }
        // Body headers: u32 kind, cdcdcdcd, name_ptr.
        let mut by_body = HashMap::new();
        let mut q = 32usize;
        while q + 16 <= d.len() {
            if u32_at(d, q + 4) == Some(0xcdcd_cdcd) {
                if let Some(&oi) = u32_at(d, q + 8).and_then(|n| by_name_off.get(&n)) {
                    if objects[oi].body_off.is_none() {
                        objects[oi].body_off = Some(q as u32);
                        objects[oi].count = u16_at(d, q + 12).unwrap_or(0);
                        by_body.insert(q as u32, oi);
                    }
                }
            }
            q += 16;
        }
        // Wave records. A wave's own name is the closest `#wave` name before its file string.
        let mut wave_names: Vec<(u32, usize)> =
            objects.iter().enumerate().filter(|(_, o)| o.kind == SndKind::Wave).map(|(i, o)| (o.name_off, i)).collect();
        wave_names.sort();
        let mut waves = Vec::new();
        let mut sound_waves: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut q = 32usize;
        while q + 0x50 <= d.len() {
            if u32_at(d, q) == Some(4) {
                let sp = u32_at(d, q + 8).unwrap_or(0) as usize;
                let snd_ptr = u32_at(d, q + 0x28).unwrap_or(0);
                let len = if sp % 16 == 0 && sp >= 32 && sp < d.len() { d[sp..].iter().take(512).position(|&b| b == 0).unwrap_or(0) } else { 0 };
                if len > 0
                    && !d[sp..].starts_with(b"audio:")
                    && u32_at(d, q + 0x24) == Some(0xcdcd_cdcd)
                    && by_name_off.contains_key(&snd_ptr)
                {
                    let idx = wave_names.partition_point(|(o, _)| (*o as usize) < sp);
                    if idx > 0 {
                        let oi = wave_names[idx - 1].1;
                        let file = String::from_utf8_lossy(&d[sp..sp + len]).into_owned();
                        let wi = waves.len();
                        waves.push(WaveObject {
                            obj: oi,
                            file,
                            whd_offset: u32_at(d, q + 0x14).unwrap_or(0),
                            sound_name_off: snd_ptr,
                            duration: f32_at(d, q + 0x18).unwrap_or(0.0),
                            x38: f32_at(d, q + 0x38).unwrap_or(0.0),
                            raw: d[q..q + 0x50].to_vec(),
                        });
                        sound_waves.entry(by_name_off[&snd_ptr]).or_default().push(wi);
                        objects[oi].body_off = Some(q as u32);
                        by_body.insert(q as u32, oi);
                    }
                }
            }
            q += 16;
        }
        let mut by_path = HashMap::new();
        for (i, o) in objects.iter().enumerate() {
            by_path.insert((o.path.clone(), kind_tag(&o.kind)), i);
        }
        let mut room_effects = Vec::new();
        let mut surfaces = Vec::new();
        for o in &objects {
            match &o.kind {
                SndKind::Other(k) if k == "roomeffect" => {
                    if let Some(r) = parse_room_effect(d, o) {
                        room_effects.push(r);
                    }
                }
                SndKind::Other(k) if k == "surfaceproperty" => {
                    if let Some(b) = o.body_off {
                        let b = b as usize;
                        let words = (0..16).filter_map(|i| u32_at(d, b + 4 * i)).collect();
                        surfaces.push(SurfaceProperty { name: o.path.clone(), words });
                    }
                }
                _ => {}
            }
        }
        SndDb { objects, waves, room_effects, surfaces, by_body, by_name_off, by_path, sound_waves, raw: d.to_vec() }
    }

    /// Resolves a body-header file offset (as stored in `ZSNDOBJ` / `Audio_*` properties) to an object.
    pub fn object_at(&self, body_off: u32) -> Option<&SndObject> {
        self.by_body.get(&body_off).map(|&i| &self.objects[i])
    }

    pub fn object_by_name_off(&self, off: u32) -> Option<&SndObject> {
        self.by_name_off.get(&off).map(|&i| &self.objects[i])
    }

    pub fn find(&self, path: &str, kind: &SndKind) -> Option<&SndObject> {
        self.by_path.get(&(path.to_ascii_lowercase(), kind_tag(kind))).map(|&i| &self.objects[i])
    }

    /// Decodes the parameter block of a `#sound` object.
    pub fn sound_params(&self, obj: &SndObject) -> Option<SoundParams> {
        if obj.kind != SndKind::Sound {
            return None;
        }
        let b = obj.body_off? as usize;
        let d = &self.raw;
        Some(SoundParams {
            range_m: f32_at(d, b + 0x18)?,
            gain_a: u16_at(d, b + 0x1c)? as i16,
            gain_b: u16_at(d, b + 0x22)? as i16,
            cone_inner: u16_at(d, b + 0x1e)?,
            cone_outer: u16_at(d, b + 0x20)?,
            volume_pct: *d.get(b + 0x27)?,
            mode: *d.get(b + 0x28)?,
            aux: *d.get(b + 0x29)?,
        })
    }

    /// Waves belonging to a `#sound` object.
    pub fn waves_of(&self, obj: &SndObject) -> Vec<&WaveObject> {
        self.by_name_off
            .get(&obj.name_off)
            .and_then(|i| self.sound_waves.get(i))
            .map(|v| v.iter().map(|&w| &self.waves[w]).collect())
            .unwrap_or_default()
    }

    pub fn room_effect(&self, path: &str) -> Option<&RoomEffect> {
        let p = path.to_ascii_lowercase();
        self.room_effects.iter().find(|r| r.name == p)
    }

    pub fn surface(&self, path: &str) -> Option<&SurfaceProperty> {
        let p = path.to_ascii_lowercase();
        self.surfaces.iter().find(|r| r.name == p)
    }

    /// Raw bytes of the file (for research tooling).
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }
}

fn parse_room_effect(d: &[u8], o: &SndObject) -> Option<RoomEffect> {
    // The name string (padded to 16) is followed by a zero line, then the EAX block.
    let name_end = o.name_off as usize + "audio:".len() + o.path.len() + "#roomeffect".len() + 1;
    let start = ((name_end + 15) & !15) + 0x10;
    let w = |i: usize| u32_at(d, start + 4 * i);
    let f = |i: usize| f32_at(d, start + 4 * i);
    let s = |i: usize| i32_at(d, start + 4 * i);
    let decay = f(8)?;
    let ok = decay.is_finite() && (0.05..=100.0).contains(&decay) && s(5)?.abs() <= 10000 && f(3)?.is_finite() && w(1)? < 64;
    if !ok {
        return None;
    }
    Some(RoomEffect {
        name: o.path.clone(),
        head: [w(0)?, w(1)?, w(2)?],
        environment_size: f(3)?,
        environment_diffusion: f(4)?,
        room: s(5)?,
        room_hf: s(6)?,
        room_lf: s(7)?,
        decay_time: f(8)?,
        decay_hf_ratio: f(9)?,
        decay_lf_ratio: f(10)?,
        reflections: s(11)?,
        reflections_delay: f(12)?,
        reverb: s(13)?,
        reverb_delay: f(14)?,
        echo_time: f(15)?,
        echo_depth: f(16)?,
        modulation_time: f(17)?,
        modulation_depth: f(18)?,
        air_absorption_hf: f(19)?,
        hf_reference: f(20)?,
        lf_reference: f(21)?,
        room_rolloff: f(22)?,
        flags: w(23)?,
    })
}
