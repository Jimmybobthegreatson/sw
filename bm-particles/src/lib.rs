//! # bm-particles
//!
//! Particle effects for the Blood Money runtime.
//!
//! * [`desc`]: plain-data, `serde` ([RON]/JSON) [`EmitterDesc`] - author your own or get one decoded from a scene.
//! * [`sim`]: deterministic CPU simulation ([`ParticleSystem`]), pooling, billboard generation
//!   (camera facing / stretched / velocity aligned) into a depth sorted [`RenderList`].
//! * [`scene`]: decodes the game's `ZParticleTemplate` / `ZParticleEmitter` nodes ([`SceneParticles`]).
//! * `render` (feature `render`, default): instanced GPU billboards drawn inside `bm-render`'s pass via
//!   `RenderHooks::draw_extra`, WGSL overridable as `Shaders/WGSL/particles.wgsl`.
//!
//! The simulation never touches the GPU; everything outside [`render`] works headless.
//! Format knowledge and confidence levels: `docs/particles.md`.
//!
//! [RON]: https://github.com/ron-rs/ron

pub mod curve;
pub mod desc;
pub mod rng;
pub mod scene;
pub mod sim;
pub mod sprites;

#[cfg(feature = "render")]
pub mod render;

pub use curve::{ColorCurve, Curve, Range};
pub use desc::{
    BillboardMode, BuiltinSprite, Burst, EmitShape, EmitterDesc, FrameMode, ParticleBlend, SimSpace, SpawnDesc, SpriteDesc, SpriteSource,
};
pub use rng::Rng;
pub use scene::{PlacedEmitter, SceneParticles};
pub use sim::{Batch, BillboardVertex, EmitterId, Particle, ParticleInstance, ParticleSystem, RenderList, View};
pub use sprites::SpriteImage;
