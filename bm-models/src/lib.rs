//! `bm-models` - renderer-agnostic CPU-side model layer for the Glacier-1 (Hitman: Blood Money) runtime.
//!
//! ```no_run
//! use bm_models::scene::{NodeKind, Scene};
//! let vfs = bm_vfs::Vfs::from_env().unwrap();
//! let scene = Scene::load(&vfs, "Scenes/M01/M01_main").unwrap();
//! for item in scene.draw_list(&[NodeKind::Static, NodeKind::Actor]) {
//!     for mesh in item.model.meshes.iter().filter(|m| m.is_lod0()) {
//!         let material = scene.materials.desc(mesh.material());           // MaterialDesc (plain data)
//!         let diffuse = material.and_then(|m| m.diffuse_texture()).and_then(|t| scene.materials.image(t));
//!         // upload mesh.vertices / mesh.indices, draw with item.world ...
//!         # let _ = diffuse;
//!     }
//! }
//! ```
//!
//! | module | contents |
//! |---|---|
//! | [`mesh`] | `Model` / `Mesh` / `SubMesh` / `VertexData` (SoA: positions, normals, tangents, uvs, colors, joints, weights) |
//! | [`decode`] | `.PRM` -> `Model` for every vertex stride (0x28, 0x24, 0x34 skinned, 0x10), water-surface grids, lazy `ModelCache` |
//! | [`scene`] | `Scene`: scene graph with world transforms, lazily decoded models, materials, node classification |
//! | [`lighting`] | decoded lights (colour / range / cone), environments (sun, ambient), zones, fog, backdrop sun |
//! | [`material`] | `.MAT` -> `MaterialDesc`, lazy RGBA texture decoding (`MaterialLibrary`) |
//! | [`scatter`] | `ZScatterObj` vegetation clumps -> sprite meshes (volume + random table decoded) |
//! | [`skeleton`] | `Skeleton`, `Pose`, skinning matrices, the stored-rotation convention |
//! | [`anim`] | `Animation` / `Track` sampling, `AnmBank` (decoded `.ANM` clip table), name index |
//! | [`anm_decode`], [`rig`] | `.ANM` stream decoder; humanoid rig parameters -> bone transforms |
//! | [`gltf_export`], [`obj`] | `.glb` / OBJ exporters (verification + modder tool) |
//! | [`import`] | `.glb` -> `Model` importer (`Model::from_glb`), TEX texture injection |
//! | [`prm_write`] | write a `Model` back into a `.PRM` (in-place replacement / new PRM) |
//! | [`math`] | `Mat4` (column-major), `Aabb`, quaternion helpers |
//!
//! Format notes and open questions: `docs/models.md`, `docs/formats.md`.

pub mod anim;
pub mod anm_decode;
pub mod decode;
pub mod gltf_export;
pub mod import;
pub mod lighting;
pub mod material;
pub mod math;
pub mod mesh;
pub mod obj;
pub mod prm_write;
pub mod rig;
pub mod scatter;
pub mod scene;
pub mod skeleton;

pub use anim::{AnmBank, Animation, Track};
pub use material::{MaterialDesc, MaterialLibrary};
pub use math::{Aabb, Mat4};
pub use mesh::{Mesh, Model, SubMesh, VertexData, VertexLayout};
pub use scene::{NodeKind, Scene};
pub use skeleton::{Pose, Skeleton};
