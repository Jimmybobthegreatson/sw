//! Scene level model access: scene graph + world transforms + lazily decoded models + materials.

use std::sync::Arc;

use bm_formats::anm::Anm;
use bm_formats::io::FormatError;
use bm_formats::loader::{LoadError, SceneFiles};
use bm_formats::loc::Loc;
use bm_formats::prm::Prm;
use bm_formats::prp::Value;
use bm_formats::scene::{SceneGraph, SceneNode};
use bm_vfs::Vfs;

use crate::decode::ModelCache;
use crate::material::MaterialLibrary;
use crate::math::{Aabb, Mat4};
use crate::mesh::Model;
use crate::skeleton::rotation_from_stored;

/// Coarse classification of scene node types (by Glacier type id).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NodeKind {
    /// Transform-only container (`ZGROUP`, `ZROOM`).
    Group,
    /// Static render model (`ZSTDOBJ`, `0x20011c` LOD models ...).
    Static,
    /// `ZLNKOBJ`: link/proxy nodes (e.g. the `Ground` node below clothing bundles that points at a
    /// combined all-variants model). Not drawn by default.
    Link,
    /// `ZScatterObj` (vegetation / scatter instances).
    Scatter,
    /// Collision / trigger / guide volumes with a PRM model (`ZBOUND`, `ElevatorBound`, movement guides).
    Bound,
    /// `ZSHADOWMESHOBJ` - shadow volume geometry, not drawn in the color pass.
    ShadowMesh,
    /// `ZHM3Actor` / `ZHitman3` (characters; skinned models).
    Actor,
    /// `ZHM3ClothBundle`.
    ClothBundle,
    Light,
    Camera,
    Particle,
    Other,
}

impl NodeKind {
    pub fn from_type_id(t: u32) -> NodeKind {
        match t {
            0x100001 | 0x100021 => NodeKind::Group,
            0x200002 | 0x200027 | 0x20011c | 0x20041d | 0x200455 | 0x20045d | 0x10003f => NodeKind::Static,
            0x200006 => NodeKind::Link,
            0x2000ce => NodeKind::Scatter,
            0x20001c | 0x200112 | 0x200435 => NodeKind::Bound,
            0x200110 => NodeKind::ShadowMesh,
            0x2003e8 | 0x2003e9 => NodeKind::Actor,
            0x200447 => NodeKind::ClothBundle,
            0x800020 | 0x800023 | 0x800024 | 0x8000f5 => NodeKind::Light,
            0x400003 | 0x40041b | 0x400424 | 0x400443 | 0x400460 => NodeKind::Camera,
            0x2000ff | 0x2000e5 | 0x10044e => NodeKind::Particle,
            _ => NodeKind::Other,
        }
    }

    /// Kinds that are drawn in the normal color pass.
    pub fn is_visible_geometry(self) -> bool {
        matches!(self, NodeKind::Static | NodeKind::Scatter | NodeKind::Actor | NodeKind::ClothBundle)
    }
}

/// Everything known about one scene node.
#[derive(Clone, Debug)]
pub struct SceneNodeInfo {
    /// Index in [`Scene::graph`] (0 = ROOT).
    pub index: usize,
    pub name: String,
    pub type_id: u32,
    pub type_name: Option<&'static str>,
    pub kind: NodeKind,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    /// Transform relative to the parent (native Glacier space).
    pub local: Mat4,
    /// Model-to-world transform (native Glacier space).
    pub world: Mat4,
    /// PRM model chunk (prim id) rendered by this node, if any.
    pub prim_id: Option<u32>,
}

/// Prim id of any node type that follows the `Bool, I32 prim, Bool` property pattern at
/// `props[3..6]` and whose id is a real model header in the PRM.
pub fn node_prim_id(node: &SceneNode, prm: &Prm) -> Option<u32> {
    if node.props.len() < 6 {
        return None;
    }
    match (&node.props[3], &node.props[4], &node.props[5]) {
        (Value::Bool(_), Value::I32(p), Value::Bool(_)) if *p > 0 && prm.model(*p as usize).is_some() => Some(*p as u32),
        _ => None,
    }
}

/// One thing to draw: a scene node, its decoded model and its world transform.
#[derive(Clone, Debug)]
pub struct DrawItem {
    pub node: usize,
    pub model: Arc<Model>,
    /// Model-to-world matrix in native Glacier space (left-handed Y-up, centimetres).
    pub world: Mat4,
}

/// A loaded scene: graph with world transforms, lazily decoded models and materials.
pub struct Scene {
    /// Scene base path without extension (`Scenes/M01/M01_main`).
    pub base: String,
    pub graph: SceneGraph,
    pub nodes: Vec<SceneNodeInfo>,
    pub prm: Prm,
    pub materials: Arc<MaterialLibrary>,
    pub anm: Anm,
    pub loc: Option<Loc>,
    cache: ModelCache,
    bank: std::sync::OnceLock<Option<crate::anim::AnmBank>>,
    lighting: std::sync::OnceLock<crate::lighting::Lighting>,
    scatter: std::sync::Mutex<std::collections::HashMap<usize, Option<Arc<Model>>>>,
}

impl Scene {
    /// Loads and parses a scene through the VFS (mods override game files).
    pub fn load(vfs: &Vfs, base: &str) -> Result<Scene, LoadError> {
        let files = SceneFiles::load(vfs, base)?;
        Scene::from_files(files).map_err(|error| LoadError::Format { file: format!("{base}.PRP"), error })
    }

    pub fn from_files(files: SceneFiles) -> Result<Scene, FormatError> {
        let graph = files.graph()?;
        let SceneFiles { base, prm, tex, mat, anm, loc, .. } = files;
        let mut nodes: Vec<SceneNodeInfo> = Vec::with_capacity(graph.nodes.len());
        for n in &graph.nodes {
            let local = match (n.rotation, n.position) {
                (Some(r), Some(p)) => Mat4::from_rotation_translation(&rotation_from_stored(&r), p),
                (None, Some(p)) => Mat4::from_translation(p),
                (Some(r), None) => Mat4::from_rotation_translation(&rotation_from_stored(&r), [0.0; 3]),
                (None, None) => Mat4::IDENTITY,
            };
            let world = match n.parent {
                Some(p) => nodes[p].world.mul(&local),
                None => local,
            };
            nodes.push(SceneNodeInfo {
                index: n.index,
                name: n.name.clone(),
                type_id: n.type_id,
                type_name: n.type_name(),
                kind: NodeKind::from_type_id(n.type_id),
                parent: n.parent,
                children: n.children.clone(),
                local,
                world,
                prim_id: node_prim_id(n, &prm),
            });
        }
        Ok(Scene {
            base,
            graph,
            nodes,
            prm,
            materials: Arc::new(MaterialLibrary::new(mat, tex)),
            anm,
            loc,
            cache: ModelCache::default(),
            bank: std::sync::OnceLock::new(),
            lighting: std::sync::OnceLock::new(),
            scatter: Default::default(),
        })
    }

    /// Decoded model for a prim id (cached).
    pub fn model(&self, prim_id: u32) -> Option<Arc<Model>> {
        self.cache.get(&self.prm, prim_id)
    }

    /// Model of a node. `ZScatterObj` nodes get a generated sprite mesh (node space, see [`crate::scatter`]); all other nodes use the
    /// decoded PRM model of their prim id.
    pub fn node_model(&self, node: usize) -> Option<Arc<Model>> {
        let n = self.nodes.get(node)?;
        let prim = n.prim_id?;
        if n.kind == NodeKind::Scatter {
            if let Some(m) = self.scatter.lock().unwrap().get(&node) {
                return m.clone();
            }
            // built outside the lock: the builder reads the models of other (non scatter) nodes
            let built = crate::scatter::build(self, node);
            self.scatter.lock().unwrap().insert(node, built.clone());
            return built;
        }
        self.model(prim)
    }

    /// Nodes of the given kinds that have a decodable model.
    pub fn geometry_nodes<'a>(&'a self, kinds: &'a [NodeKind]) -> impl Iterator<Item = &'a SceneNodeInfo> + 'a {
        self.nodes.iter().filter(move |n| n.prim_id.is_some() && kinds.contains(&n.kind))
    }

    /// Draw list of all nodes of the given kinds that have a decodable model (graph order).
    pub fn draw_list(&self, kinds: &[NodeKind]) -> Vec<DrawItem> {
        self.geometry_nodes(kinds)
            .filter_map(|n| Some(DrawItem { node: n.index, model: self.node_model(n.index)?, world: n.world }))
            .collect()
    }

    /// Decoded lights, environments (sun / ambient), zones and fog of the scene (built on first use, see [`crate::lighting`]).
    pub fn lighting(&self) -> &crate::lighting::Lighting {
        self.lighting.get_or_init(|| crate::lighting::Lighting::build(&self.nodes, &self.graph, &self.prm))
    }

    /// The sun / ambient environments that apply at a world position (zone lookup, see [`crate::lighting::Lighting::environments_at`]).
    pub fn environments_at(&self, p: [f32; 3]) -> crate::lighting::EnvSelection {
        self.lighting().environments_at(p, &|n| self.nodes.get(n).and_then(|x| x.parent))
    }

    /// The decoded clip table of this scene's `.ANM` (parsed on first use; `None` for scenes without animations).
    pub fn anim_bank(&self) -> Option<&crate::anim::AnmBank> {
        self.bank.get_or_init(|| crate::anim::AnmBank::parse(&self.anm.raw)).as_ref()
    }

    /// Outfit value of a `ZHM3Actor` node (property 6, e.g. `1`, `5`, `9`, `13`): the variant mask of the skinned meshes
    /// of the actor's combined model that this actor wears (see [`crate::mesh::Mesh::in_outfit`] and `docs/models.md`).
    pub fn actor_outfit(&self, node: usize) -> Option<u8> {
        use bm_formats::prp::Value;
        let n = self.graph.nodes.get(node)?;
        if n.type_id != 0x2003e9 {
            return None;
        }
        match n.props.get(6)? {
            Value::I32(v) if (1..=255).contains(v) => Some(*v as u8),
            _ => None,
        }
    }

    /// Animation database name of a `ZHM3Actor` (`M01_Don`, `M01_TouristFemale`, ...; property 8 without the
    /// `anmcol:animationdatabase#` prefix).
    pub fn actor_database(&self, node: usize) -> Option<String> {
        use bm_formats::prp::Value;
        let n = self.graph.nodes.get(node)?;
        if n.type_id != 0x2003e9 {
            return None;
        }
        match n.props.get(8)? {
            Value::Str(s) => Some(s.rsplit('#').next().unwrap_or(s).to_string()),
            _ => None,
        }
    }

    /// Slash separated path of a node (`Group/Sub/Name`).
    pub fn path(&self, node: usize) -> String {
        self.graph.path(node)
    }

    /// World space bounding box of a node's model (from the PRM header box).
    pub fn world_bbox(&self, node: usize) -> Option<Aabb> {
        let n = self.nodes.get(node)?;
        let m = self.node_model(node)?;
        Some(m.bbox.transformed(&n.world))
    }

    /// All descendants of `node` (depth first, including itself).
    pub fn subtree(&self, node: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack = vec![node];
        while let Some(i) = stack.pop() {
            out.push(i);
            for &c in self.nodes[i].children.iter().rev() {
                stack.push(c);
            }
        }
        out
    }
}
