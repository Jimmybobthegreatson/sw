//! # bm-script - modding for the Blood Money Rust runtime
//!
//! * [`package`] - mod folders / zips with a `mod.toml` [`manifest`], discovery, enable list,
//!   dependency resolution + load order, asset-conflict reports, VFS mounting (`ModManager`).
//! * [`runtime`] - sandboxed Luau scripting (one state per mod, time/memory limits, hot reload).
//! * [`host`] - the [`ScriptHost`] trait `bm-game` implements; [`mock::MockHost`] for tests.
//! * [`events`] - event catalogue and dispatch types.
//! * [`data`] / [`schema`] - merge-patch data modding with schema validation.
//! * [`spec`] - API description used to generate LuaLS stubs and the docs.
//! * [`cli`] - `bm mods ...` command implementation.
//!
//! See `docs/modding.md` for the user guide and API reference.

pub mod bindings;
pub mod cli;
pub mod data;
pub mod events;
pub mod host;
pub mod manifest;
pub mod mock;
pub mod package;
pub mod runtime;
pub mod schema;
pub mod spec;

pub use data::{DataReport, DataStore, merge_patch};
pub use events::{DispatchOutcome, ErrorKind, Event, ScriptError};
pub use host::{
    EntityId, EntityInfo, EntityQuery, HostError, HostResult, LogLevel, ParticleParams, RayHit, ScriptHost, SoundParams,
    SpawnRequest, Vec3,
};
pub use manifest::{Diagnostic, ModManifest, Severity};
pub use mock::{HostCall, MockHost};
pub use package::{
    AssetConflict, ConflictReport, DiscoveredMod, ModConfig, ModDirs, ModError, ModManager, MountReport, Problem,
    ProblemKind, ResolvedMod, ResolvedMods,
};
pub use runtime::{Limits, LoadReport, ModStatus, ReloadKind, ReloadReport, RuntimeConfig, ScriptRuntime};

/// Version of the scripting API (semver). Mods declare a requirement in `api_version`.
/// Minor bumps add functions/events; a major bump may break scripts.
pub const API_VERSION: &str = "1.0.0";

pub fn api_version() -> semver::Version {
    semver::Version::parse(API_VERSION).expect("valid API_VERSION")
}
