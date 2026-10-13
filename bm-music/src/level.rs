//! Reads the original music tables of a level from the game files:
//!
//! * the 64-slot track table (`audio:_sounddefinitionsbloodmoney/music/music_mXX#def` in the level's
//!   `.SND`): slot index -> `#sound` -> bank file (confidence H, see `docs/music.md`);
//! * the `MusicLocationDef` zone nodes of the scene graph (zone id, name, room list).

use crate::error::Result;
use crate::modding::level_id;
use bm_formats::{buf::Buf, gms::Gms, prp::{Prp, Value}, scene::SceneGraph};
use bm_sound::snd_db::{SndDb, SndKind};
use bm_sound::SoundLibrary;
use bm_vfs::Vfs;

pub const MUSIC_DEF_PREFIX: &str = "_sounddefinitionsbloodmoney/music/";
pub const SLOT_COUNT: usize = 64;

/// One occupied entry of the level's music table.
#[derive(Clone, Debug)]
pub struct SlotTrack {
    pub slot: u32,
    /// `.SND` sound path, e.g. `music/exploration` (level specific ones look like `music/m01/action2`).
    pub sound: String,
    /// Bank sound name (`Music/Exploration.wav`) when the sound resolves to a bank entry.
    pub file: Option<String>,
    /// `#sound` volume byte (+0x27). Plausibly a per-track trim in percent (confidence M).
    pub volume_pct: u8,
    /// `#sound` +0x28 (0 = bed, 1 / 2 = see docs; confidence ?).
    pub mode: u8,
    pub aux: u8,
    pub duration: Option<f32>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    /// Raw `.WHD` flags (0x8280 = PCM music with a lip/stream header, 0x180 mono stem or radio mix).
    pub flags: Option<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoneKind {
    Music,
    /// Zone name says "No Music" / "Stop Music" / "Silent".
    Silent,
}

/// A `MusicLocationDef` node.
#[derive(Clone, Debug)]
pub struct ZoneInfo {
    pub id: i32,
    /// Node name without the audio-zip prefix, e.g. `Zone_3__Cellar`.
    pub name: String,
    /// Room names (last path segment) covered by the zone.
    pub rooms: Vec<String>,
    pub kind: ZoneKind,
}

#[derive(Clone, Debug)]
pub struct LevelMusic {
    pub scene_base: String,
    /// `M01`, `M06`, `hideout`...
    pub level: String,
    /// `_sounddefinitionsbloodmoney/music/music_m01` if present.
    pub def_name: Option<String>,
    /// Always `SLOT_COUNT` entries; `None` = empty slot.
    pub slots: Vec<Option<SlotTrack>>,
    pub zones: Vec<ZoneInfo>,
}

fn u32_at(d: &[u8], p: usize) -> Option<u32> {
    d.get(p..p + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Classifies a zone by its node name.
pub fn zone_kind(name: &str) -> ZoneKind {
    let n = name.to_ascii_lowercase().replace([' ', '-'], "_");
    if ["nomusic", "no_music", "stopmusic", "stop_music", "silent"].iter().any(|k| n.contains(k)) {
        ZoneKind::Silent
    } else {
        ZoneKind::Music
    }
}

fn zone_name(node: &str) -> String {
    node.rsplit('!').next().unwrap_or(node).to_string()
}

fn room_name(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_string()
}

impl LevelMusic {
    /// Reads slot table and zones of `scene_base` (e.g. `Scenes/M01/M01_main`). `lib` must contain the
    /// scene's bank (`SoundLibrary::open_level`).
    pub fn load(vfs: &Vfs, lib: &SoundLibrary, scene_base: &str, with_zones: bool) -> Result<LevelMusic> {
        let mut out = LevelMusic { scene_base: scene_base.to_string(), level: level_id(scene_base), def_name: None, slots: vec![None; SLOT_COUNT], zones: Vec::new() };
        for b in 0..lib.bank_count() as u16 {
            if let Some(db) = lib.snd(b) {
                if out.read_slots(db, lib) {
                    break;
                }
            }
        }
        if with_zones {
            out.zones = read_zones(vfs, scene_base).unwrap_or_default();
        }
        Ok(out)
    }

    fn read_slots(&mut self, db: &SndDb, lib: &SoundLibrary) -> bool {
        let Some(def) = db.objects.iter().find(|o| matches!(o.kind, SndKind::Other(_)) && o.path.starts_with(MUSIC_DEF_PREFIX)) else {
            return false;
        };
        let Some(body) = def.body_off.map(|b| b as usize) else { return false };
        let raw = db.raw();
        let Some(tp) = u32_at(raw, body + 0x14).map(|v| v as usize) else { return false };
        if tp == 0 || tp >= body || body - tp > SLOT_COUNT * 4 {
            return false;
        }
        self.def_name = Some(def.path.clone());
        for i in 0..(body - tp) / 4 {
            let Some(v) = u32_at(raw, tp + i * 4) else { break };
            if v == 0 {
                continue;
            }
            let Some(so) = db.object_at(v) else { continue };
            if so.kind != SndKind::Sound {
                continue;
            }
            let params = db.sound_params(so);
            let file = db
                .waves_of(so)
                .first()
                .map(|w| w.file.clone())
                .or_else(|| lib.find(&format!("{}.wav", so.path)).and_then(|id| lib.info(id)).map(|i| i.name));
            let info = file.as_ref().and_then(|f| lib.find(f)).and_then(|id| lib.info(id));
            if i < SLOT_COUNT {
                self.slots[i] = Some(SlotTrack {
                    slot: i as u32,
                    sound: so.path.clone(),
                    file: file.clone(),
                    volume_pct: params.map(|p| p.volume_pct).unwrap_or(100),
                    mode: params.map(|p| p.mode).unwrap_or(0),
                    aux: params.map(|p| p.aux).unwrap_or(0),
                    duration: info.as_ref().map(|i| i.duration),
                    sample_rate: info.as_ref().map(|i| i.sample_rate),
                    channels: info.as_ref().map(|i| i.channels),
                    flags: info.as_ref().map(|i| i.flags),
                });
            }
        }
        true
    }

    pub fn slot(&self, n: usize) -> Option<&SlotTrack> {
        self.slots.get(n).and_then(|s| s.as_ref())
    }

    /// Bank sound names per slot (input of `GameResolver`).
    pub fn slot_files(&self) -> Vec<Option<String>> {
        self.slots.iter().map(|s| s.as_ref().and_then(|t| t.file.clone())).collect()
    }
}

/// Reads all `MusicLocationDef` controllers of a scene.
pub fn read_zones(vfs: &Vfs, scene_base: &str) -> Option<Vec<ZoneInfo>> {
    let g = Gms::parse(&vfs.open(&format!("{scene_base}.GMS")).ok()?).ok()?;
    let b = Buf::parse(&vfs.open(&format!("{scene_base}.BUF")).ok()?).ok()?;
    let p = Prp::parse(&vfs.open(&format!("{scene_base}.PRP")).ok()?).ok()?;
    let graph = SceneGraph::build(&g, &b, &p).ok()?;
    let mut zones: Vec<ZoneInfo> = Vec::new();
    for nd in &graph.nodes {
        for c in nd.controllers.iter().filter(|c| c.name == "MusicLocationDef") {
            let id = match c.props.get(1) {
                Some(Value::I32(i)) => *i,
                _ => continue,
            };
            let rooms: Vec<String> = match c.props.first() {
                Some(Value::Container(v)) | Some(Value::Array(v)) => v
                    .iter()
                    .filter_map(|x| if let Value::Str(s) = x { Some(room_name(s)) } else { None })
                    .collect(),
                _ => Vec::new(),
            };
            let name = zone_name(&nd.name);
            let kind = zone_kind(&name);
            zones.push(ZoneInfo { id, name, rooms, kind });
        }
    }
    zones.sort_by_key(|z| z.id);
    Some(zones)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_classification() {
        assert_eq!(zone_kind("Zone_0__No_Music"), ZoneKind::Silent);
        assert_eq!(zone_kind("Zone_0__NoMusic"), ZoneKind::Silent);
        assert_eq!(zone_kind("Zone_1__StopMusic"), ZoneKind::Silent);
        assert_eq!(zone_kind("Zone_00__Silent_Rooms"), ZoneKind::Silent);
        assert_eq!(zone_kind("Zone_3__Cellar"), ZoneKind::Music);
        assert_eq!(zone_name("M01/M01_Audio_Music.zip!Zone_3__Cellar"), "Zone_3__Cellar");
        assert_eq!(room_name("ROOT\\M01/M01_CHARACTERS.zip\\M01_Hacienda_Room_101"), "M01_Hacienda_Room_101");
    }
}
