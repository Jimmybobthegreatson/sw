# bm-models - model layer

Crate `bm-models` (depends on `bm-vfs`, `bm-formats`) turns the raw `.PRM` / `.PRP` / `.GMS` / `.MAT` /
`.TEX` data into renderer-agnostic CPU data: meshes, skeletons, materials, world transforms,
exporters (glTF / OBJ) and an importer + PRM writer for mods. Nothing in it touches a GPU API.
No game data is stored in the repo; every number below was derived from the user's own files and is
asserted by `bm-models/tests/real_files.rs` (skipped when the game directory is absent).

Confidence legend as in `formats.md`: **H** verified on all/most shipped data by an automated test,
**M** consistent with samples, **L** guess, **?** unknown.

## Public API (what the renderer needs)

```rust
use bm_models::{Scene, NodeKind, Model, Mesh, MaterialDesc, Skeleton, Pose, Mat4};

let vfs = bm_vfs::Vfs::from_env()?;                           // mods: vfs.add_mod_dir(..)
let scene = Scene::load(&vfs, "Scenes/M01/M01_main")?;        // graph + PRM + MAT/TEX + ANM (lazy models)

scene.nodes[i]            // SceneNodeInfo { index, name, type_id, type_name, kind: NodeKind, parent, children,
                          //                 local: Mat4, world: Mat4, prim_id: Option<u32> }
scene.draw_list(&[NodeKind::Static, NodeKind::Scatter, NodeKind::Actor])  // Vec<DrawItem { node, model: Arc<Model>, world }>
scene.model(prim_id) / scene.node_model(node)                 // Option<Arc<Model>>, decoded on first use, cached
scene.materials.desc(mat_id)      // Option<Arc<MaterialDesc>>   (plain data)
scene.materials.image(tex_id)     // Option<Arc<Image>>          (RGBA8, mip 0, decoded on first use, cached)
scene.world_bbox(node) / scene.subtree(node) / scene.path(node) / scene.graph (bm_formats SceneGraph)
```

Model data (all in native Glacier space, see *Conventions*):

```rust
Model   { prim_id, bbox: Aabb, meshes: Vec<Mesh>, skeleton: Option<Arc<Skeleton>> }
Mesh    { layout: VertexLayout, vertices: VertexData, indices: Vec<u32> /*triangle list*/, submeshes: Vec<SubMesh>,
          source_chunk, variant, group, flags, extra_words, bone_palette, palette_words, name }
          // helpers: is_lod0(), lod_mask(), variant_mask(), material(), is_skinned(), bbox(), triangle_count()
SubMesh { first_index, index_count, material /* .MAT instance id */ }
VertexData { positions: Vec<[f32;3]>, normals, tangents: Vec<[f32;4]> /*xyz + handedness*/, uvs: Vec<[f32;2]>,
             colors: Vec<[u8;4]> /*RGBA*/, joints: Vec<[u16;4]> /*skeleton bone ids*/, weights: Vec<[f32;4]> }
```

Materials:

```rust
MaterialDesc { id, name, shader: Option<String>, render_state: RenderState, textures: Vec<TextureBinding>,
               colors: BTreeMap<String,[f32;4]>, floats: BTreeMap<String,Vec<f32>>, flags: BTreeMap<String,bool> }
RenderState  { blend: BlendMode, opacity, alpha_test, alpha_ref, cull: CullMode, z_bias, z_offset, fog_enabled }
// helpers: diffuse_texture(), normal_texture(), specular_mask_texture(), illumination_texture(), diffuse_color(),
//          flag("BumpEnabled"), is_shadow_only()
```

Skeletons / animation:

```rust
Skeleton { bones: Vec<Bone { name, parent, bind_local: Mat4, bind_world: Mat4 }>, facial_poses }
skeleton.world_matrices(&pose) / .inverse_bind() / .skinning_matrices(&pose) -> Vec<Mat4>   // p' = sum w_i * M[j_i] * p
Pose { bones: Vec<BoneTransform { translation, rotation /*xyzw*/, scale }> },  Pose::bind(&skeleton)
Animation { name, duration, looping, tracks: Vec<Track { bone, translations, rotations, scales }> }
animation.sample(&skeleton, t) -> Pose        // bones without a track keep the bind pose
anim::anm_index(&scene.anm) -> AnmIndex       // databases, clip paths, "anim:" object names, skeleton names
```

Export / import / mod writing:

```rust
gltf_export::export_scene_glb(&scene, &ExportOptions) -> GlbExport { glb: Vec<u8>, stats }   // opts.root = Some(node) for a subtree
gltf_export::export_model_glb(&model, &scene.materials, &opts)
obj::export_scene_obj(&scene, &opts) / obj::export_model_obj(..)
Model::from_glb(&bytes) / import::import_glb(&bytes, &ImportOptions) -> ImportedModel { model, materials, warnings }
prm_write::write_model(&mut prm, prim_id, &model)   // replace a model in place;  prm_write::build_prm(&model) for a new file
import::add_rgba_texture(&mut tex, name, w, h, &rgba) -> Option<texture id>
```

`cargo run --release -p bm-models --example export_scene -- Scenes/M01/M01_main m01.glb [--node <name|index>] [--tex 512]`
and `--example scene_tree` (hierarchy browser with triangle counts) are the command line front ends.

## Conventions

* Glacier is **left-handed, Y-up**, units are **centimetres** (the player is ~180 units tall). Stored triangle
  winding is **clockwise** (Direct3D): for 99.5 % of all triangles `cross(b-a, c-a) . vertex_normal < 0` **H** (99.54 % of 530 000 triangles in M01).
* Mesh data in `bm-models` stays in this native space; node `world` matrices are native too.
  Renderers using a right-handed convention should mirror Z or flip winding (see glTF below).
* **Stored rotation matrices** (`.PRP` base property, `.GMS` rotation block, PRM bone blocks) are 9 floats in
  *reversed axis order*: stored row 0 is the image of the Z axis, row 1 of Y, row 2 of X; an unrotated object stores
  `0,0,1, 0,1,0, 1,0,0` and all stored matrices have determinant -1. The proper column-vector rotation is
  `R = (J*S)^T = S^T*J` (`R[i][j] = S[2-j][i]`), implemented as `skeleton::rotation_from_stored` / `rotation_to_stored`.
  Evidence **H**: with this convention `world == parent_world * local` holds with mean error 0.000 cm over all 100
  bones of the player skeleton (every one of the 6 alternatives is off by 7-23 cm); in M01 all 8 792 node matrices
  become proper rotations; rendering the Hacienda / the whole M01 hillside in Blender gives continuous terrain and
  walls (`export_scene` + the viewer check described below).
  Node composition is `world = parent.world * T(position) * R`.

## `.PRM` model decoding (extends `formats.md`)

Chunk graph, in bm-models terms (all pointers are chunk indices):

```
model header (0x40 B, w0 = 0x00070100)  w3 = 2 (static) / 0x10b.. (skinned) / 0x132 (water)   w4 = parts chunk
                                        w5 = object count, w6 = list of object chunks, f32 w8..13 = bbox
parts chunk (16 B)   [0, skeleton_list_chunk or 0, 0, 0]       -> skeleton block (skinned models only)
object chunk         0x40 B (standard), 0x80 B (PosColor10 shadow meshes), 0xb0 B (water surface; first word 0xb0101)
   w0   variant (0x80189 / 0x80181 / 0x80089 / 0x80182 / 0x8018b / 0x80489 ...; meaning of the bits ?)
   w3   group word: bits 16..24 = LOD mask (bit0 = most detailed; 0xff all), bits 24..32 = outfit value of skinned
        character meshes (1..3 kind + 4 * clothes set, see `docs/anim.md`), low 16 bits 3 = skinned, 0/2/0x300 = static  **H**
   w4   ((material instance id + 1) << 16) | ?   (0 = no material, see `docs/anim.md`)       w5 = 2/0/4     w6 = color/tint word (?)
   w7   flags (0x80, 0x880, 0x400080, 0x442080 ...: visibility / shadow / LOD style bits ?)
   w10  model-ref chunk -> w[0] = primitive descriptor chunk       w11 = 1/2/3/0x33 (?)
   w14/w15 (skinned) = number of palette ranges / palette chunk;  w14..w29 of 0x80 B objects = 4 x (1.0,0,0,0,0)
primitive descriptor { vertex_count, vertex_chunk, 5*(triangles-1), index_chunk }     (word 2 formula verified on 25/25 samples)
index chunk          u16 1, u16 count, u16 indices[count]                              (triangle list)
vertex chunk         vertex_count * stride bytes, padded to 16
```

* Stride detection: `chunk_size == vertex_count * stride` (remainder < 16). Sprite quads and a few shared
  pools reference a chunk holding more vertices than `vertex_count` (e.g. 8 verts for a 4 vertex quad); then the
  stride is the one dividing the chunk evenly (`0x28` if the pad at +36 is `0xcdcdcdcd`) and the first
  `vertex_count` vertices are used **M**. Skinned objects are recognised by `w3 & 0xffff == 3`.
* Objects reachable from M01 scene nodes (counted per node, so shared models count repeatedly): 11 854 `Standard28`,
  14 220 `Skinned34`, 201 `Simple24`, 167 `PosColor10` (162 of them below `ZSHADOWMESHOBJ`).

### Vertex layouts **H** (positions, normals, tangent frames, uvs, colors, weights all validated)

| stride | layout |
|---|---|
| 0x28 `Standard28` | `f32 pos[3]` +0, packed normal +12, color +16, `f32 uv[2]` +20, packed tangent +28, packed binormal +32, pad +36 (`0xcdcdcdcd` or 0) |
| 0x24 `Simple24`   | `f32 pos[3]`, `f32 normal[3]` +12, color +24, `f32 uv[2]` +28 (no tangent frame) |
| 0x34 `Skinned34`  | `f32 pos[3]`, `f32 weight[3]` +12 (4th = 1 - sum), `u8 bone[4]` +24, packed normal +28, color +32, `f32 uv[2]` +36, packed tangent +44, packed binormal +48 |
| 0x10 `PosColor10` | `f32 pos[3]`, color +12 (shadow volume / collision style geometry, untextured) |

* **Packed direction** = 4 bytes in `D3DCOLOR` order `[B, G, R, x]`; component = `byte / 127.5 - 1`
  (x = R, y = G, z = B). Unit length within quantisation; tangent . normal mean 0.007 / 0.036 (skinned / standard), and
  `cross(n, t) . binormal` gives the handedness sign (`tangents[i][3]`).
* **Color** = `[B, G, R, A]` bytes (D3DCOLOR), exposed as RGBA. These are baked vertex colors (AO/lighting); the
  glTF exporter omits them by default because they darken the albedo.
* UVs are Direct3D style (origin top-left) = glTF, no flip. They routinely exceed `[0,1]` (tiling).
* Weights: the 0x34 layout stores three float weights; the 4th is implicit. All 110 835 weight-1 samples of M01 pass
  `sum == 1, w >= 0` and bind to the right anatomy (below).

### Skinning **H**

* Vertex bone bytes are *local palette indices x 3* (register offsets of a 3x4 constant matrix per bone). Every skinned
  object owns a palette chunk (`w15`): pairs `(size_bytes, offset_bytes)` terminated by a zero pair; a pair names
  the consecutive skeleton bones `offset/12 .. (offset+size)/12`, and the palette is the concatenation of all ranges
  (max 26 bones per object in shipped data - the engine skins in vertex shader constants). `bone_palette` holds the
  expanded table; `VertexData::joints` are already remapped to skeleton bone indices.
  Example: eyes object `[0x18, 0x228]` -> bones 46, 47 = `FA_RIGHT_EYE`, `FA_LEFT_EYE`; its vertices split 32/32
  between local 0 and 1.
* Validation (test `skinned_meshes_bind_to_the_right_bones`): 97.5 % of weight-1 vertices lie within 40 cm of their
  bone joint (shuffled-bone control: 40 %), 0 of 110 835 out of range.
* Skinned meshes of one model contain *all* outfits and LODs of a character: e.g. 191 objects for the player,
  distinguished by `group` (`lod_mask`, `variant_mask`); an actor wears exactly the outfit value stored in its `ZHM3Actor`
  (property 6, `Scene::actor_outfit`, `Model::resolve_outfit`, `docs/anim.md` section 6); the exporters default to LOD0 + outfit 1.

### Skeleton block **H** (names, parents, bind matrices) / **?** (rest)

`parts[1]` -> list chunk `[bone_count, records, world, local, ...]`:

* records chunk: `bone_count x 64 B`: `+0 f32[3]` (?), `+12 i32 parent` (-1 = root), `+16 f32[3]` (?), `+28 char name[32]`,
  `+60 u32` flags (?). Parents always precede children. Player skeleton: 100 bones starting `GROUND, PELVIS, SPINE,
  SPINE_1, SPINE_2, NECK, HEAD, ...` (+ `FA_*` facial bones, `xtra_*` muscle helper bones); NPC models 96/97 bones, some
  props 2 or 26.
* world chunk: `bone_count x 48 B` = 9 stored rotation floats + translation (T-pose, Y up, head at ~162 cm).
* local chunk: same layout relative to the parent. `world == parent_world * local` verified (tolerance 0.05 cm).
* Remaining list entries (not used): `[4] 3x3 matrices (100x36 B)`, `[5] 256 B` and `[8]..[10]` index tables (u8/u16 bone
  orderings and 12-byte offsets, the data behind the palette ranges), `[6] 100x28 B` (quaternion+position form?),
  `[7] 1216 B` (per-bone constraint records?), `[11] -> [bank, offsets, 0x15c, map, 0x37]` = **55 facial pose** deltas
  (matches the 55 pose names `Neutral, A_Mouth_Widen, ...` of the `.ANM` skeleton block), `[18]` inverse bind matrices
  (3x4, translation column holds `-R^-1 t`), `[19]` byte offsets `3*12*bone` per bone. `Skeleton::inverse_bind()` is
  computed from the world matrices instead.

### Special objects

| first word | meaning |
|---|---|
| `0x90109` (32 words) | **ZScatterObj** procedural scatter (grass/foliage): no vertex chain. `w4>>16` = material (`ScatterMaterial`), `w11/w13` -> a 368 x 16 B cell table (3 small ints + running index, then random floats) and a 368 B per-cell type table, `w14 = 0x168`, bounding box +-8000 cm. `Scene::model()` returns `None` for these (190 nodes in M01). Not decoded (**?**). |
| `0xb0101` (44 words) | **WaterSurface**: `w10 x w11` grid cells over `f32 w12 x w13` cm starting at `(w14, w16)` in the plane `y = 0`; the other words are wave/color parameters (kept in `Mesh::extra_words`). Decoded as a flat grid mesh named `water_surface` **M**. |
| prim id 0 | `ZSTDOBJ` nodes with `Bool, I32(0), Bool` carry **no model** (bone/attachment helpers such as `W_Bone_BoltAction_01`). This - not an alternate layout with first word `0x000b0a01` - is what the "~200 undecoded ZSTDOBJ" of the first recon were (205 in M01). |

With the above, **5 252 / 5 252** non-scatter model nodes of M01 decode (every `ZSTDOBJ`, `ZLNKOBJ`, `ZBOUND`,
`ZSHADOWMESHOBJ`, `ZHM3Actor`, ...), and the first 12 scenes decode > 90 % of all distinct models (the rest is scatter).

### Which node types carry a model

Pattern `props[3..6] == (Bool, I32 prim_id, Bool)` with a valid model header at `prim_id` (not 0): `ZSTDOBJ 0x200002`,
`ZLNKOBJ 0x200006` (proxy nodes below clothing bundles pointing at the combined all-variants character model; kind
`Link`, not drawn by default), `ZBOUND 0x20001c` / `0x200112` / movement guides `0x200435` (bounds, kind `Bound`),
`ZSHADOWMESHOBJ 0x200110`, `ZScatterObj 0x2000ce`, `ZHM3Actor 0x2003e9` / `ZHitman3 0x2003e8` (characters, skinned),
`ZHM3ClothBundle 0x200447`, `0x20041d`, `0x200455`, `0x20045d` (posters, handles ...). `NodeKind` classifies them.

## Materials (`material.rs`)

`MaterialDesc` is built from the `BIND` children of the `.MAT` instance:

* `RSTA` -> `RenderState`: `BENA` (blend enable) + `BMOD` -> `BlendMode` (`Opaque`, `TRANS`, `TRANS_ON_OPAQUE`,
  `ADD`, `ADD_BEFORE_TRANS`, `ADD_ON_OPAQUE`, `TRANSADD_ON_OPAQUE`, `SHADOW`, `STATICSHADOW`), `OPAC`, `ATST`/`AREF`
  alpha test, `CULL` (`DontCare`, `OneSided`, `TwoSided`), `ZBIA`, `ZOFF`, `FENA`.
* `TEXT` -> `TextureBinding { slot, texture_id, enabled, tile_u, tile_v }`. Slots seen over all scenes: `mapDiffuse` (every
  material), `mapNormal`, `mapSpecularMask`, `mapIllumination`, `mapEnvironment`, `mapReflectionMask`,
  `mapReflectionFallOff`, `mapParallax`, `mapTranslucency`. Tile modes `TILED`, `MIRRORED`, `NONE`/empty (clamp).
* `COLO` -> `colors` (`v4DiffuseColor`, `v4SpecularColor`, `v4IlluminationColor`, `v4ReflectionColor`, `v4RimLightColor`,
  `v4SubSurfaceColor`), `FLTV` -> `floats` (`v4SpecularPower`, `vSpecularLevel`, `v4BumpScale`, `fRimLightPower`, wobble
  parameters ...), `BOOL` -> `flags` (`BumpEnabled`, `SpecularEnabled`, `ReflectionEnabled`, `IlluminationEnabled`,
  `RimLightingEnabled`, `SubSurfaceEnabled`, `ParallaxEnabled`, `ScrollEnabled`, `AlphaFadeEnabled`, `WobbleEnabled`).
  Only entries with `ENAB == 1` are kept.
* `MaterialLibrary::image(id)` decodes texture mip 0 to RGBA8 on first use (DXT1/3/5, RGBA, I8, U8V8, paletted).
  `U8V8` normal maps decode as `(u, v, 255)` (unsigned bias, flat = 127/128); the glTF exporter does not write normal maps (needs a Z
  reconstruction + channel convention check). In M01 97 % of model material uses name an enabled diffuse texture
  (1494 of 1538 sampled) and 95 % of those decode (1414); the rest use texture ids that are not plain images
  (cube/id lists) or formats left to the TEX decoder.
* Mapping to the original `.fx` effects (`Standard13.fx`, `Skin20.fx`, ...) is still unknown; the renderer should
  implement a generic shader driven by `MaterialDesc`.

## Animation / `.ANM`

Verified structure (**M**): header `"MNA\0", size|0x80000000, size, 0x4c, 5, 0x800009e0 (flagged pointer), 0x9bc, 1,
0x269 (count), 0x9ac`, then a `0..count-1` index table, then one `anmcol:animationdatabase#<Name>` object per
character/weapon (the name string appears twice; 36 databases in M01 such as `M01_Don`, `SMG_Mp5`, `Hitman`) followed by
`(id, 0)` tables; later `(id, offset)` pair tables, a pool of animation-set paths (`/Movement/Run/Forward`,
`/Sit/GetUpLeftFast`, `/FirstPerson/UB_1P_Hold_AirRifle` ...), the key data (~4 MB), and a tail string pool with the
skeleton's bone names (`GROUND`, `PELVIS`, ... `xtra_upv_neck_mov`) and 883 animation object names
(`anim:Male_Reg/Male_Reg_Run#Run_Backward`, `anim:Hero/Hero_Sneak#Sneak_Ambient`, ...). `anim::anm_index` returns all
of this.

**Key data decoded** (`docs/anim.md`): the clip headers, bone id list and clip names sit at the end of the file, the key data
pool is a bit packed block coded stream (`anm_decode`) of humanoid rig parameters that `rig::RigSolver` turns into bone
transforms. API: `Scene::anim_bank()` -> `AnmBank { clips, bone_names, find, find_suffix, search, raw_clip, animation,
animation_with }`; `Animation { tracks, duration, looping, root_motion }`, `animation.sample(&skeleton, t) -> Pose`.
The `(id, offset)` tables of the animation databases are bookkeeping and not needed to play clips.

`.OCT` / `.RMC` / `.RMI` were not needed (no collision / navigation consumer yet) and stay as documented in
`formats.md`; `ZBOUND`/`ZBoxPrimitive` bounds are available as ordinary nodes (`NodeKind::Bound`).

## glTF / OBJ export

* `export_scene_glb`: node hierarchy preserved (only branches that contain exported geometry), one glTF mesh per distinct
  model (instances share it), materials with `baseColorTexture` (PNG, deduplicated, samplers from `TILU/TILV`),
  `alphaMode` (MASK for `ATST`, BLEND for alpha textures with blend modes), `doubleSided` for `TwoSided`/blended,
  `extras` with the original `.MAT` id / blend mode / shader and per-primitive PRM chunk, variant and flag words,
  skins for skinned instances (joint nodes in bind pose + inverse bind matrices, `JOINTS_0`/`WEIGHTS_0`).
* Coordinate conversion (default): `z' = -z` for positions, normals, tangents (`w` flipped) and node matrices
  (`C*M*C`), then a root `scale` node of 0.01 (cm -> m). **Indices are written unchanged**: a Z mirror turns Glacier's
  clockwise faces into glTF's counter-clockwise ones (test `glb_export_..._winding`: > 98 % agree with the normals).
* Defaults skip shadow-only materials (`SHADOW`, `STATICSHADOW`), `PosColor10` meshes, non-LOD0 meshes and, for skinned
  meshes, outfits other than bit 0 (all configurable in `ExportOptions`).
* Size (textures capped to 512 px): M01 whole level 72 MB / 1.4 s, M01 locations 30 MB, the Hacienda interior 14 MB;
  all 66 shipped scene archives under `Scenes/<dir>/` export without errors. The tests re-parse a Hacienda export with the `gltf` crate and check
  primitive/triangle counts (stored and instanced), the world bounding box against `stats.bbox`, and winding.
* **Visual check** (Blender 5.2, headless Workbench render of the exported `.glb`): M01 shows the continuous jungle
  hillside, Hacienda and cliffs; the Hacienda interior shell shows aligned walls, windows and balconies; the film
  star NPC appears as a clean bind-pose man in a white suit. Reproduce with
  `blender -b --python render.py -- scene.glb out.png` (importer: `bpy.ops.import_scene.gltf`).
* OBJ: `export_scene_obj` bakes world transforms, writes `v/vt/vn/f`, one `usemtl` per submesh and a `.mtl` with diffuse
  color / `map_Kd` (textures returned as PNG bytes).

## Importer and loose-file model replacement

`Model::from_glb(bytes)` / `import_glb(bytes, &ImportOptions)`:

* flattens the default scene, bakes node transforms, converts glTF -> Glacier (Z mirror, x100) - the exact inverse of
  the exporter, so an exported model re-imports with identical vertex/triangle counts and bbox (test
  `glb_roundtrip_import_and_prm_write`);
* materials: `extras.bm_material` (written by our exporter) -> `.MAT` id, else `ImportOptions::material_ids[name]`, else
  `default_material`; embedded PNG base color images are returned in `ImportedModel::materials` and can be added to the
  scene `.TEX` with `import::add_rgba_texture` (uncompressed RGBA, full mip chain, new id 128..2047);
* missing normals/tangents are generated (area-weighted smooth normals, per-vertex tangents from UVs);
* skinned import needs the target skeleton (`ImportOptions::skeleton`): joints are mapped by node name, weights are
  renormalised;
* limits: GLB only (no external buffers), triangle lists only, PNG textures only, <= 65 535 vertices per object.

`prm_write::write_model(&mut prm, prim_id, &model)` replaces a model *in place*: new vertex/index/descriptor/model-ref/
object chunks are appended and the original model header is re-pointed (object count, object list, bbox), so scene nodes
and every other model keep working. `prm.to_bytes()` can be written as a loose
`<mod dir>/Scenes/<level>/<scene>.PRM`; `bm_vfs::Vfs::add_mod_dir` resolves it before the archive copy (test
`replacement_model_works_as_loose_file_mod`). Details and limits:

* vertex buffers are re-encoded with 8-bit packed normals/tangents (< 1 % error), UV/position floats are exact;
* skinned replacements keep the original skeleton block (the `parts` chunk of the replaced model) and rebuild the bone
  palette from the used bones - at most 26 distinct bones per object (error `TooManyBones`) - the importer does not split
  meshes by bone count;
* unknown object words (`w6` tint, `w11`, flags) are taken from the mesh (`variant`, `group`, `flags`, `extra_words`)
  or defaulted for imported meshes; whether the original executable accepts all defaults was not testable here
  (no game launch in this environment) - the layout is byte-identical to shipped objects for everything we understand;
* `build_prm` creates a standalone PRM for the Rust runtime (static meshes only).
* the original engine loads PRMs from the scene ZIP; a mod needs a loader that honours loose files (the Rust runtime
  does; for the original `.exe` the PRM has to be re-packed into the scene archive).

## Tests

`cargo test -p bm-models` (skips real-file tests without the game): decode ratios, all four vertex strides, normal/tangent
unit-ness and winding, skinning/binding closed loops, skeleton hierarchy, material/texture resolution, glb export
parse-back (counts, bbox, winding, skins), OBJ face counts, glb -> Model -> PRM -> Model round trip, skinned PRM rewrite,
loose-file override through the VFS, ANM index, whole-scene decode.

## Open questions

* Facial pose bank: decoded (`docs/anim.md` section 7); remaining skeleton-block tables.
* Meaning of object words `w0` variants, `w6`, `w7` bits, `w11`, `model header w3`; the exact meaning of the LOD bits `0x7c` / `0x80`.
* `ZScatterObj` procedural data (cell tables), `ZHM3ClothBundle` cloth data.
* Material class -> effect mapping; the engine's shader constants for `v4BumpScale`, rim lighting, sub-surface, etc.
* Whether `unk1` of PRM chunks (reference counts) matters for the engine when appending chunks.
