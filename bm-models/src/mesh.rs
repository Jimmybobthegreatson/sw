//! Renderer-agnostic mesh data.
//!
//! Coordinate conventions (see `docs/models.md`): Glacier-1 is **left-handed, Y-up**, units are
//! centimetres, triangles are stored in the engine's native (Direct3D) winding. All data in this
//! module is kept in native Glacier space; exporters / renderers convert.

use crate::math::Aabb;

/// Vertex strides found in `.PRM` vertex chunks and the layout they map to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VertexLayout {
    /// 0x28 bytes: standard lit mesh (pos, packed normal/tangent/binormal, color, uv).
    Standard28,
    /// 0x24 bytes: pos, float normal, color, uv (no tangent frame).
    Simple24,
    /// 0x34 bytes: skinned (pos, 3 blend weights, 4 bone indices, normal, color, uv, tangent, binormal).
    Skinned34,
    /// 0x10 bytes: position + color only (shadow / collision style meshes).
    PosColor10,
}

impl VertexLayout {
    pub fn from_stride(stride: usize) -> Option<VertexLayout> {
        Some(match stride {
            0x28 => VertexLayout::Standard28,
            0x24 => VertexLayout::Simple24,
            0x34 => VertexLayout::Skinned34,
            0x10 => VertexLayout::PosColor10,
            _ => return None,
        })
    }

    pub fn stride(self) -> usize {
        match self {
            VertexLayout::Standard28 => 0x28,
            VertexLayout::Simple24 => 0x24,
            VertexLayout::Skinned34 => 0x34,
            VertexLayout::PosColor10 => 0x10,
        }
    }
}

/// Structure-of-arrays vertex data. Optional attributes are empty vectors when the source layout
/// does not carry them; otherwise every present vector has `positions.len()` entries.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VertexData {
    pub positions: Vec<[f32; 3]>,
    /// Unit normals (empty for `PosColor10`).
    pub normals: Vec<[f32; 3]>,
    /// xyz tangent + w handedness (`+1`/`-1`, bitangent = cross(n, t) * w). Only `Standard28` and `Skinned34`.
    pub tangents: Vec<[f32; 4]>,
    /// Texture coordinates, top-left origin (D3D convention, same as glTF).
    pub uvs: Vec<[f32; 2]>,
    /// Vertex color RGBA8.
    pub colors: Vec<[u8; 4]>,
    /// Skinned only: bone indices into the model's [`Skeleton::bones`](crate::skeleton::Skeleton::bones)
    /// (already resolved through the object's bone palette; see [`Mesh::bone_palette`]).
    pub joints: Vec<[u16; 4]>,
    /// Skinned only: 4 blend weights (the 4th is derived as `1 - w0 - w1 - w2`).
    pub weights: Vec<[f32; 4]>,
}

impl VertexData {
    pub fn len(&self) -> usize {
        self.positions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    pub fn is_skinned(&self) -> bool {
        !self.joints.is_empty()
    }

    pub fn bbox(&self) -> Aabb {
        Aabb::from_points(&self.positions)
    }
}

/// A draw range of a [`Mesh`] with one material. Triangle list.
#[derive(Clone, Debug, PartialEq)]
pub struct SubMesh {
    /// Offset into `Mesh::indices`.
    pub first_index: u32,
    pub index_count: u32,
    /// `.MAT` instance id (0 = none).
    pub material: u32,
}

/// One vertex buffer + index buffer + material ranges. For decoded PRM data a `Mesh` is exactly one
/// PRM *object* (one primitive, one material, one `SubMesh`).
#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    pub layout: VertexLayout,
    pub vertices: VertexData,
    /// Triangle-list indices into `vertices` (native winding).
    pub indices: Vec<u32>,
    pub submeshes: Vec<SubMesh>,
    /// PRM object chunk index this mesh came from (0 for imported meshes).
    pub source_chunk: u32,
    /// PRM object flag word 0 (`0x80189`, `0x80089`, ... variant bits, meaning mostly unknown).
    pub variant: u32,
    /// PRM object word 3. Bits 16..24 look like a LOD visibility mask (bit 0 = most detailed level;
    /// `0xff` = all levels), bits 24..32 an outfit/variant mask of skinned character meshes, the low
    /// 16 bits are `3` for skinned and `0`/`2`/`0x300` for static meshes. See `docs/models.md`.
    pub group: u32,
    /// PRM object flag word 7 (`0x80`, `0x880`, `0x400080`, ...; likely visibility/LOD/shadow bits).
    pub flags: u32,
    /// Extra object words past the first 16 (bigger "0x80 byte" objects carry four 1.0/0 groups here).
    pub extra_words: Vec<u32>,
    /// Static 0x40-byte objects: object word 15, the *part id* of modular models (the Custom_* weapons: one id per
    /// `W_Bone_*` part such as frame / slide / clip / silencer, `docs/models.md`). 0 = not part of a modular model.
    pub part: u32,
    /// Skinned meshes: the object's bone palette, `palette[local_index] = skeleton bone index`.
    /// The vertex stream stores *local* indices (pre-multiplied by 3, hardware constant register
    /// offsets); `VertexData::joints` is already remapped through this table.
    pub bone_palette: Vec<u16>,
    /// Skinned meshes: raw palette chunk words `(size_bytes, offset_bytes)` pairs (bones are
    /// `offset/12 .. (offset+size)/12`), kept for lossless writing.
    pub palette_words: Vec<u32>,
    pub name: String,
}

impl Mesh {
    /// LOD mask (object word 3 bits 16..24).
    pub fn lod_mask(&self) -> u8 {
        (self.group >> 16) as u8
    }

    /// True when the mesh is part of the most detailed LOD (or carries no LOD information).
    pub fn is_lod0(&self) -> bool {
        let m = self.lod_mask();
        m == 0 || m & 1 != 0
    }

    /// Outfit / variant mask of skinned meshes (object word 3 bits 24..32; 0 = all).
    pub fn variant_mask(&self) -> u8 {
        (self.group >> 24) as u8
    }

    /// Whether a skinned character mesh belongs to the outfit `mask` (an actor's outfit value, see
    /// [`Model::resolve_outfit`]): meshes without a variant mask are common to all outfits, the others are
    /// replicated once per outfit value and are drawn only for the exactly matching one.
    pub fn in_outfit(&self, mask: u8) -> bool {
        let v = self.variant_mask();
        v == 0 || v == mask
    }

    pub fn material(&self) -> u32 {
        self.submeshes.first().map(|s| s.material).unwrap_or(0)
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn bbox(&self) -> Aabb {
        self.vertices.bbox()
    }

    pub fn is_skinned(&self) -> bool {
        self.vertices.is_skinned()
    }

    /// Triangles of one submesh.
    pub fn submesh_indices(&self, s: &SubMesh) -> &[u32] {
        &self.indices[s.first_index as usize..(s.first_index + s.index_count) as usize]
    }
}

/// A PRM model: what a `ZSTDOBJ` node's "prim id" refers to.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    /// PRM chunk index of the model header (the prim id used by the scene nodes).
    pub prim_id: u32,
    /// Bounding box from the PRM model header (cm, model space).
    pub bbox: Aabb,
    pub meshes: Vec<Mesh>,
    /// Present for skinned character/prop models.
    pub skeleton: Option<std::sync::Arc<crate::skeleton::Skeleton>>,
}

impl Model {
    pub fn triangle_count(&self) -> usize {
        self.meshes.iter().map(|m| m.triangle_count()).sum()
    }

    pub fn vertex_count(&self) -> usize {
        self.meshes.iter().map(|m| m.vertices.len()).sum()
    }

    /// Bounding box recomputed from the vertex data.
    pub fn computed_bbox(&self) -> Aabb {
        self.meshes.iter().fold(Aabb::EMPTY, |b, m| b.union(&m.bbox()))
    }

    /// Distinct outfit values (non-zero variant masks) of the skinned meshes, ascending.
    pub fn outfit_masks(&self) -> Vec<u8> {
        let mut v: Vec<u8> = self.meshes.iter().filter(|m| m.is_skinned()).map(|m| m.variant_mask()).filter(|&m| m != 0).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// The outfit value to draw for an actor that asks for `want`: `want` itself when the model has meshes for it,
    /// else the lowest "normal" outfit (low two bits `01`), else the lowest mask; 0 for models without variants.
    pub fn resolve_outfit(&self, want: u8) -> u8 {
        let masks = self.outfit_masks();
        if masks.contains(&want) {
            return want;
        }
        masks.iter().copied().find(|m| m & 3 == 1).or_else(|| masks.first().copied()).unwrap_or(0)
    }

    /// Distinct material ids used.
    pub fn materials(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.meshes.iter().flat_map(|m| m.submeshes.iter().map(|s| s.material)).collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}
