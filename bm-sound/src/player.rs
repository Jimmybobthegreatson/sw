//! High level glue: play sounds of a [`SoundLibrary`] on an [`AudioEngine`] by id, name or `.SND` object.

use crate::engine::AudioEngine;
use crate::library::{Result, SoundError, SoundId, SoundInfo, SoundLibrary};
use crate::mixer::{Bus, Spatial, VoiceHandle, VoiceParams};
use crate::snd_db::SndKind;
use std::sync::Arc;

/// Per-play options; everything optional falls back to a sensible default derived from the sound.
#[derive(Clone, Debug, Default)]
pub struct PlayOptions {
    /// World position in native centimetres (`None` = 2D).
    pub position: Option<[f32; 3]>,
    pub volume: Option<f32>,
    pub pitch: Option<f32>,
    pub looping: bool,
    pub bus: Option<Bus>,
    pub priority: Option<u8>,
    pub reverb_send: Option<f32>,
    /// Full-volume / silent distances in metres for positional sounds (default 5 m / 60 m).
    pub min_max_m: Option<(f32, f32)>,
}

/// Default bus of a bank entry (by name prefix): `Dialog*`/`Character/Expressions` -> voice, `Music*` -> music, `OSD/` -> UI,
/// `LevelSetup/` ambience -> ambient, everything else -> sfx.
pub fn default_bus(info: &SoundInfo) -> Bus {
    let n = info.name.to_ascii_lowercase();
    if n.starts_with("music") || matches!(info.flags, 0x180 | 0x8280) {
        Bus::Music
    } else if n.starts_with("dialog") || n.starts_with("character/expressions") {
        Bus::Voice
    } else if n.starts_with("osd/") {
        Bus::Ui
    } else if n.starts_with("levelsetup") || n.contains("ambien") {
        Bus::Ambient
    } else {
        Bus::Sfx
    }
}

pub struct SoundPlayer {
    pub lib: Arc<SoundLibrary>,
    pub engine: Box<dyn AudioEngine>,
    rng: u64,
}

impl SoundPlayer {
    pub fn new(lib: Arc<SoundLibrary>, engine: Box<dyn AudioEngine>) -> SoundPlayer {
        SoundPlayer { lib, engine, rng: 0x9E37_79B9_7F4A_7C15 }
    }

    /// Reseeds the pseudo random wave picker (deterministic tests).
    pub fn seed(&mut self, s: u64) {
        self.rng = s | 1;
    }

    fn next_u32(&mut self) -> u32 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }

    fn params(&self, info: Option<&SoundInfo>, o: &PlayOptions, base_volume: f32) -> VoiceParams {
        let bus = o.bus.or_else(|| info.map(default_bus)).unwrap_or(Bus::Sfx);
        let (min_m, max_m) = o.min_max_m.unwrap_or((5.0, 60.0));
        VoiceParams {
            bus,
            volume: o.volume.unwrap_or(1.0) * base_volume,
            pitch: o.pitch.unwrap_or(1.0),
            looping: o.looping,
            priority: o.priority.unwrap_or(match bus {
                Bus::Voice => 200,
                Bus::Ui => 220,
                Bus::Music => 250,
                Bus::Ambient => 60,
                Bus::Sfx => 128,
            }),
            spatial: o.position.map(|p| Spatial::from_meters(p, min_m, max_m)),
            pan: 0.0,
            reverb_send: o.reverb_send.unwrap_or(0.0),
            occlusion: 0.0,
            fade_in: 0.0,
        }
    }

    /// Plays a bank sound. `Ok(None)` when the engine rejected the voice (voice limit).
    pub fn play_id(&mut self, id: SoundId, o: &PlayOptions) -> Result<Option<VoiceHandle>> {
        let info = self.lib.info(id).ok_or_else(|| SoundError::NotFound(format!("{id:?}")))?;
        let pcm = self.lib.pcm(id)?;
        let p = self.params(Some(&info), o, 1.0);
        Ok(self.engine.play(pcm, p))
    }

    /// Plays by (case-insensitive) name, including mod-only loose sounds.
    pub fn play_name(&mut self, name: &str, o: &PlayOptions) -> Result<Option<VoiceHandle>> {
        if let Some(id) = self.lib.find(name) {
            return self.play_id(id, o);
        }
        let pcm = self.lib.pcm_by_name(name)?;
        let p = self.params(None, o, 1.0);
        Ok(self.engine.play(pcm, p))
    }

    /// Plays a `#sound` object of the level's `.SND` (`audio:` path, e.g. `impacts_bullet/glass/impact_glass_1`):
    /// picks one of its waves at random and applies the object's volume.
    pub fn play_object(&mut self, bank: u16, path: &str, o: &PlayOptions) -> Result<Option<VoiceHandle>> {
        let lib = self.lib.clone();
        let db = lib.snd(bank).ok_or_else(|| SoundError::NotFound(format!("no .SND for bank {bank}")))?;
        let obj = db.find(path, &SndKind::Sound).ok_or_else(|| SoundError::NotFound(format!("sound object {path}")))?;
        let waves = db.waves_of(obj);
        if waves.is_empty() {
            return Err(SoundError::NotFound(format!("sound object {path} has no waves")));
        }
        let w = waves[self.next_u32() as usize % waves.len()];
        let vol = db.sound_params(obj).map(|p| p.volume()).unwrap_or(1.0);
        let id = lib.find(&w.file).ok_or_else(|| SoundError::NotFound(w.file.clone()))?;
        let info = lib.info(id);
        let pcm = lib.pcm(id)?;
        let p = self.params(info.as_ref(), o, vol);
        Ok(self.engine.play(pcm, p))
    }
}
