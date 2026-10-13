//! Sound-related scene nodes: `ZSNDOBJ` emitters, `Audio_*` / `Sound*` / `Music*` controllers and the
//! room-reverb lookup. Built from a loaded [`bm_models::Scene`] plus the level's [`SndDb`].
//!
//! Confidence (details in `docs/sound.md`):
//! * `ZSNDOBJ` (`0x200012`) `props[6]` is the file offset of a `#sound`/`#set`/`#group` body in the level's
//!   `.SND`: **H** (resolves for all emitters that are not `0`).
//! * `Audio_PlaySound` = `[snd offset, trigger events...]`, `Audio_AttachToGeom` = `[snd offset]`,
//!   `Audio_Definitions` = `[def offset, 0]`, `MetalDetectorAudioDef` = `[loop offset, found offset]`: **H** for the
//!   structure, **M** for the semantics (taken from the names and values).
//! * `SoundLimiterFade`, `MusicLocationDef`, `SoundSpline*`, `SoundVolumetric*`: structure **H**, semantics **M/?**.

use crate::snd_db::{SndDb, SndKind};
use bm_formats::prp::Value;
use bm_models::{Mat4, Scene};

pub const TYPE_ZSNDOBJ: u32 = 0x200012;
pub const TYPE_ZROOM: u32 = 0x100021;

/// A resolved reference into the `.SND` object graph.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundRef {
    pub offset: u32,
    pub kind: SndKind,
    /// `audio:` path without kind suffix, lower case.
    pub path: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SoundBehaviour {
    /// `Audio_PlaySound`: play `sound` when one of `events` fires (`Activate`, `Deactivate`, `ProjectileHit` ...).
    PlayOnEvent { sound: Option<SoundRef>, events: Vec<String> },
    /// `Audio_AttachToGeom`: the geometry owns a sound (offset 0 = none).
    AttachToGeom { sound: Option<SoundRef> },
    /// `Audio_Definitions`: per-level sound definition tables (`#def`).
    Definitions { def: Option<SoundRef> },
    /// `Audio_Set3DMusic`: the emitter is a positional music source.
    Music3D,
    /// `SoundLimiterFade`: audible only while the listener is in one of `rooms`; cross-fades over
    /// `fade_in`/`fade_out` seconds (`M`: order and meaning of the two floats inferred from 3.0/3.0 and 1.0/4.0).
    LimiterFade { rooms: Vec<String>, fade_in: f32, fade_out: f32, flags: [bool; 2] },
    /// `MusicLocationDef`: music zone `zone` covering `rooms` (`M`).
    MusicLocation { rooms: Vec<String>, zone: i32 },
    /// `SoundSpline`: sound that moves along a spline node (`?` floats).
    Spline { spline: String, params: Vec<f32> },
    /// `SoundSplineFollow`: random / moving point sounds on a spline (`mode` = `RANDOM`, `RANDOM_AND_MOVE`).
    SplineFollow { spline: String, mode: String, rate: f32, count: i32, flag: bool },
    /// `SoundVolumetricSource` (11 values, `?`).
    VolumetricSource { params: Vec<f32> },
    /// `SoundVolumetricRandom` (3 floats `?`: e.g. 3000, 100, 1500).
    VolumetricRandom { params: Vec<f32> },
    /// `MetalDetectorAudioDef`: idle loop and "found weapon" sound.
    MetalDetector { idle: Option<SoundRef>, found: Option<SoundRef> },
}

/// A scene node that carries sound information.
#[derive(Clone, Debug)]
pub struct SoundNode {
    pub node: usize,
    pub name: String,
    pub type_id: u32,
    /// World position in native units (centimetres, left-handed Y-up).
    pub position: [f32; 3],
    pub world: Mat4,
    /// For `ZSNDOBJ`: the sound object it plays (`props[6]`).
    pub sound: Option<SoundRef>,
    pub behaviours: Vec<SoundBehaviour>,
}

impl SoundNode {
    pub fn is_emitter(&self) -> bool {
        self.type_id == TYPE_ZSNDOBJ
    }
}

#[derive(Clone, Debug, Default)]
pub struct SoundScene {
    pub nodes: Vec<SoundNode>,
    /// `(room node index, room name)` of every `ZROOM`.
    pub rooms: Vec<(usize, String)>,
}

fn resolve(db: &SndDb, off: i32) -> Option<SoundRef> {
    if off <= 0 {
        return None;
    }
    db.object_at(off as u32).map(|o| SoundRef { offset: off as u32, kind: o.kind.clone(), path: o.path.clone() })
}

fn strings(v: &Value) -> Vec<String> {
    match v {
        Value::Str(s) | Value::Token(s) => vec![s.clone()],
        Value::Container(c) | Value::Array(c) => c.iter().flat_map(strings).collect(),
        Value::StrArray(a) => a.clone(),
        _ => vec![],
    }
}

fn floats(v: &[Value]) -> Vec<f32> {
    v.iter()
        .filter_map(|x| match x {
            Value::F32(f) => Some(*f),
            Value::I32(i) => Some(*i as f32),
            Value::Bool(b) => Some(*b as u8 as f32),
            _ => None,
        })
        .collect()
}

fn last_segment(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

impl SoundScene {
    pub fn from_scene(scene: &Scene, db: &SndDb) -> SoundScene {
        let mut out = SoundScene::default();
        for n in &scene.graph.nodes {
            let info = &scene.nodes[n.index];
            if n.type_id == TYPE_ZROOM {
                out.rooms.push((n.index, n.name.clone()));
            }
            let mut b = Vec::new();
            for c in &n.controllers {
                let p = &c.props;
                let sref = |i: usize| p.get(i).and_then(|v| v.as_i32()).and_then(|o| resolve(db, o));
                let bh = match c.name.as_str() {
                    "Audio_PlaySound" => SoundBehaviour::PlayOnEvent {
                        sound: sref(0),
                        events: p.iter().skip(1).flat_map(strings).collect(),
                    },
                    "Audio_AttachToGeom" => SoundBehaviour::AttachToGeom { sound: sref(0) },
                    "Audio_Definitions" => SoundBehaviour::Definitions { def: sref(0) },
                    "Audio_Set3DMusic" => SoundBehaviour::Music3D,
                    "SoundLimiterFade" => SoundBehaviour::LimiterFade {
                        rooms: p.first().map(strings).unwrap_or_default(),
                        fade_in: p.get(2).and_then(|v| v.as_f32()).unwrap_or(0.0),
                        fade_out: p.get(3).and_then(|v| v.as_f32()).unwrap_or(0.0),
                        flags: [
                            matches!(p.get(4), Some(Value::Bool(true))),
                            matches!(p.get(5), Some(Value::Bool(true))),
                        ],
                    },
                    "MusicLocationDef" => SoundBehaviour::MusicLocation {
                        rooms: p.first().map(strings).unwrap_or_default(),
                        zone: p.get(1).and_then(|v| v.as_i32()).unwrap_or(-1),
                    },
                    "SoundSpline" => SoundBehaviour::Spline {
                        spline: p.first().map(strings).and_then(|s| s.into_iter().next()).unwrap_or_default(),
                        params: floats(&p[p.len().min(1)..]),
                    },
                    "SoundSplineFollow" => SoundBehaviour::SplineFollow {
                        spline: p.first().map(strings).and_then(|s| s.into_iter().next()).unwrap_or_default(),
                        mode: p.get(1).map(strings).and_then(|s| s.into_iter().next()).unwrap_or_default(),
                        rate: p.get(2).and_then(|v| v.as_f32()).unwrap_or(0.0),
                        count: p.get(3).and_then(|v| v.as_i32()).unwrap_or(0),
                        flag: matches!(p.get(4), Some(Value::Bool(true))),
                    },
                    "SoundVolumetricSource" => SoundBehaviour::VolumetricSource { params: floats(p) },
                    "SoundVolumetricRandom" => SoundBehaviour::VolumetricRandom { params: floats(p) },
                    "MetalDetectorAudioDef" => SoundBehaviour::MetalDetector { idle: sref(0), found: sref(1) },
                    _ => continue,
                };
                b.push(bh);
            }
            let sound = if n.type_id == TYPE_ZSNDOBJ { n.props.get(6).and_then(|v| v.as_i32()).and_then(|o| resolve(db, o)) } else { None };
            if n.type_id == TYPE_ZSNDOBJ || !b.is_empty() {
                out.nodes.push(SoundNode {
                    node: n.index,
                    name: n.name.clone(),
                    type_id: n.type_id,
                    position: info.world.translation(),
                    world: info.world,
                    sound,
                    behaviours: b,
                });
            }
        }
        out
    }

    /// All `ZSNDOBJ` emitters.
    pub fn emitters(&self) -> impl Iterator<Item = &SoundNode> {
        self.nodes.iter().filter(|n| n.is_emitter())
    }

    /// Emitters within `radius` (native units) of `pos`.
    pub fn emitters_near(&self, pos: [f32; 3], radius: f32) -> Vec<&SoundNode> {
        let r2 = radius * radius;
        self.emitters()
            .filter(|n| (0..3).map(|i| (n.position[i] - pos[i]).powi(2)).sum::<f32>() <= r2)
            .collect()
    }

    /// The EAX reverb preset for a room node name such as `M01_Hacienda_Room_101` (the effect objects are named
    /// `roomeffects/<level>/<group>/<room>`, matched on the last path segment, case-insensitively).
    pub fn room_effect<'a>(&self, db: &'a SndDb, room_name: &str) -> Option<&'a crate::snd_db::RoomEffect> {
        let r = last_segment(room_name).to_ascii_lowercase();
        db.room_effects.iter().find(|e| last_segment(&e.name) == r)
    }
}
