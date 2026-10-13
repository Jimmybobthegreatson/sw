//! # bm-game - simulation core of the Blood Money Rust runtime
//!
//! Renderer independent: it produces [`RenderState`] / [`UiState`] every frame that `bm-app` feeds to
//! `bm-render` / `bm-ui`, and implements `bm_script::ScriptHost` ([`GameHost`]).
//!
//! ```no_run
//! use bm_game::{Game, LoadOptions, PlayerInput};
//! let vfs = std::sync::Arc::new(bm_vfs::Vfs::from_env().unwrap());
//! let mut game = Game::load(vfs, "Scenes/M01/M01_main", &LoadOptions::default()).unwrap();
//! let input = PlayerInput { move_y: 1.0, ..Default::default() };
//! for _ in 0..600 { game.tick(&input); }          // 10 s of fixed 60 Hz simulation
//! let rs = game.render_state();                    // camera, posed characters, lights, particles
//! println!("{} entities, alert {:?}", rs.entities.len(), game.max_npc_alert());
//! ```
//!
//! | module | contents |
//! |---|---|
//! | [`world`] | entity store (SoA components), hitscan, damage / kill / pickup / door logic, request + fx queues |
//! | [`level`] | `load_level`: scene graph -> world (collision, actors, doors, pickups, lights, emitters, hierarchy) |
//! | [`collision`], [`controller`], [`nav`] | triangle BVH, kinematic capsule controller, lazily built walk grid + A* |
//! | [`player`], [`ai`], [`mission`] | third-person player + camera + weapons, perception + replaceable `AiBrain`, RON objectives |
//! | [`anim`] | `AnimationProvider` (bind pose / procedural gaits / decoded clips) |
//! | [`host`], [`game`] | `ScriptHost` implementation and the `Game` facade (fixed timestep, events, scripts) |
//! | [`audio`], [`fx`] | `bm-sound` / `bm-music` and `bm-particles` glue |
//! | [`state`] | `RenderState`, `UiState` |
//! | [`data`] | weapon / mission definitions (RON, embedded defaults, VFS overrides) |
//!
//! Notes and honest gaps: `docs/game.md`.

pub mod ai;
pub mod areas;
pub mod anim;
pub mod audio;
pub mod collision;
pub mod controller;
pub mod data;
pub mod doors;
pub mod face;
pub mod fx;
pub mod game;
pub mod host;
pub mod level;
pub mod math;
pub mod mission;
pub mod nav;
pub mod objectives;
pub mod player;
pub mod props;
pub mod recoil;
pub mod save;
pub mod rooms;
pub mod sim;
pub mod state;
pub mod stealth;
pub mod world;

pub use ai::{AiBrain, AiCommand, AiView, AlertState, MoveSpeed, Role, SimpleBrain};
pub use anim::{AnimRequest, AnimState, AnimationProvider, BindPoseProvider, ClipProvider, ProceduralProvider, SceneClipProvider};
pub use audio::{AlertLevel, AudioDirector};
pub use collision::{CollisionWorld, Surface, Tri};
pub use data::{GameData, MissionDef, WeaponDef};
pub use fx::FxDirector;
pub use game::{EventRecord, Game};
pub use host::{GameHost, HostCommand};
pub use level::{GameError, Level, LevelStats, LoadOptions, load_level};
pub use math::{Rng, Vec3, v3};
pub use mission::{MissionState, MissionStatus};
pub use player::{Camera, PlayerInput, Prompt};
pub use save::{SAVE_VERSION, SaveError, SaveGame};
pub use sim::TICK_DT;
pub use props::{PropDraw, PropSlot};
pub use state::{RenderEntity, RenderLight, RenderState, UiMessage, UiState};
pub use world::{DoorComp, DoorMotion, EntityId, FxEvent, Kind, Request, World};
