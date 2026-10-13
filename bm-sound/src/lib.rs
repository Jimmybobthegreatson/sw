//! `bm-sound` - Hitman: Blood Money audio: sound banks, `.SND` object database, scene sound nodes, a 3D
//! mixer with a headless renderer and loose-file modding. See `docs/sound.md`.
pub mod decode;
#[cfg(feature = "device")]
pub mod device;
pub mod engine;
pub mod library;
pub mod lip;
pub mod mixer;
pub mod pcm;
pub mod player;
pub mod reverb;
pub mod scene;
pub mod snd_db;

pub use engine::{AudioEngine, MixerEngine, NullEngine};
pub use library::{SoundError, SoundId, SoundInfo, SoundLibrary};
pub use lip::LipSync;
pub use mixer::{Bus, Listener, Mixer, PcmSource, Spatial, VoiceHandle, VoiceParams, VoiceUpdate};
pub use pcm::{AudioStats, Pcm};
pub use player::{PlayOptions, SoundPlayer};
pub use reverb::ReverbParams;
pub use scene::SoundScene;
pub use snd_db::SndDb;
