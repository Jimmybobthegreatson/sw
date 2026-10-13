//! `bm-music` - data-driven dynamic music for the Blood Money runtime.
//!
//! * [`def`]: plain-data (RON/JSON) description of tracks, layered cues, states, transitions, zones,
//!   stingers and ducking.
//! * [`source`] / [`feed`] / [`segment`]: random-access sources (streamed `pc_eng.str` PCM, loose
//!   ogg/flac/wav), loop-aware gapless generation (intro, loop points, loop crossfade, resampling) and
//!   a read-ahead ring so the audio thread never blocks on I/O.
//! * [`conductor`] / [`system`]: the state machine + mixer behind a `PcmSource` that plays on
//!   `Bus::Music` of any `bm_sound::AudioEngine`, or renders offline.
//! * [`level`] / [`defaults`]: decodes the original music tables of a level (`.SND` 64-slot track table,
//!   `MusicLocationDef` zones) and builds a playable default definition.
//! * [`modding`]: loose replacement files and custom definition files.
//!
//! See `docs/music.md` for the format findings and the authoring reference.

pub mod analysis;
pub mod conductor;
pub mod curve;
pub mod def;
pub mod defaults;
pub mod error;
pub mod feed;
pub mod level;
pub mod modding;
pub mod segment;
pub mod source;
pub mod system;

pub use conductor::{CueStatus, MusicEvent, MusicStats, MusicStatus};
pub use curve::{Curve, FadeDef};
pub use def::{
    CueDef, DuckingDef, EventAction, Format, IntensityDef, LayerDef, MusicDef, SourceRef, StateDef, StemDef, StingerDef, SyncMode, TrackDef, TransitionDef,
    TransitionRule,
};
pub use defaults::{default_bank_def, default_level_def, default_level_def_with, load_level_def, merge_mod_definitions, slot_role, DefaultsOptions, LevelMusicSet};
pub use error::{MusicError, Result};
pub use level::{LevelMusic, SlotTrack, ZoneInfo, ZoneKind};
pub use source::{AudioSource, GameResolver, MemSource, MemoryResolver, OpenContext, StrPcmSource, TrackResolver};
pub use system::{MissionEvent, MusicMixer, MusicOptions, MusicSystem};
