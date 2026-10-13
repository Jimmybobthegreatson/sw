use thiserror::Error;

#[derive(Debug, Error)]
pub enum MusicError {
    #[error("vfs: {0}")]
    Vfs(#[from] bm_vfs::VfsError),
    #[error("sound: {0}")]
    Sound(#[from] bm_sound::SoundError),
    #[error("decode: {0}")]
    Decode(#[from] bm_sound::decode::DecodeError),
    #[error("parse {what}: {msg}")]
    Parse { what: String, msg: String },
    #[error("unknown {kind} `{name}`")]
    Unknown { kind: &'static str, name: String },
    #[error("invalid music definition: {}", .0.join("; "))]
    Invalid(Vec<String>),
    #[error("track `{0}` cannot be opened: {1}")]
    Track(String, String),
    #[error("audio engine refused the music stream (voice limit)")]
    NoVoice,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, MusicError>;
