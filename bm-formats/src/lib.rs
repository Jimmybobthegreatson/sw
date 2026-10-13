//! `bm-formats` - parsers (and where possible serialisers) for the Glacier-1 file formats used by
//! Hitman: Blood Money. All parsers work on in-memory byte slices; use `bm-vfs` to obtain the bytes.
//!
//! See `docs/formats.md` for the byte-level documentation and confidence levels.
//!
//! | module | extension | state |
//! |---|---|---|
//! | [`gms`] | `.GMS` | container + geometry table (hierarchy comes from the PRP) |
//! | [`buf`] | `.BUF` | name pool |
//! | [`prp`] | `.PRP` | full op stream, byte-exact round trip, scene tree |
//! | [`scene`] | GMS+BUF+PRP | joined scene graph |
//! | [`gameplay`] | scene nodes + LOC | typed doors, routes, actors, trigger volumes, pickups, mission objectives |
//! | [`loader`] | all of the above | `SceneFiles::load(&vfs, "Scenes/M01/M01_main")` |
//! | [`prm`] | `.PRM` | chunk container, model/object/primitive headers |
//! | [`tex`] | `.TEX` | full decode (DXT1/3/5, RGBA, I8, U8V8, paletted), writer |
//! | [`mat`] | `.MAT` | material instances + property records |
//! | [`light`] | PRM light chunks | light / environment records (colour, range, cone) |
//! | [`loc`] | `.LOC` | full parse + writer |
//! | [`anm`] | `.ANM` | header, tables, name pools (animation payload not decoded) |
//! | [`oct`] | `.OCT` | block list of packed bounding-volume trees (leaf boxes decoded) |
//! | [`roommap`] | `.RMC .RMI` | room box records + quantised tree, point queries |
//! | [`sgp`], [`snd`], [`misc`] | `.SGP .SND .ZGF .SUP .SGD` | structural parsing |

pub mod anm;
pub mod buf;
pub mod gameplay;
pub mod gms;
pub mod io;
pub mod light;
pub mod loader;
pub mod loc;
pub mod mat;
pub mod misc;
pub mod oct;
pub mod prm;
pub mod prp;
pub mod roommap;
pub mod scene;
pub mod sgp;
pub mod snd;
pub mod tex;

pub use io::{FormatError, Result};
