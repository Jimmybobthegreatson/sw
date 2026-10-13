# Glacier-1 (Hitman: Blood Money) file formats

Everything here was derived from the shipped files (hex analysis + round-trip tests in
`bm-formats/tests/real_files.rs`) and cross-checked against community notes (ReGlacier `ReHitman` /
`HBM_GMSTool`, AlexKimov `hitman-file-formats` 010-editor templates for TEX/LOC, Cryect's PRM converter
notes for the PRM chunk graph). No third-party code or game data is copied; all code is original.

Confidence legend: **H** high (verified on every shipped scene, usually by byte-exact round trip),
**M** medium (verified on samples / by consistency checks), **L** low (guess), **?** unknown.
All integers are little-endian. "Scene" = one `Scenes/**/<name>.ZIP`; formats appear once per scene as
`<name>.<EXT>` (see `docs/vfs.md`).

## How the files fit together

```
GMS  geometry table (type id, name offset into BUF, header words)         ┐ joined by DFS order
BUF  name pool                                                             │  (bm_formats::scene)
PRP  property stream: hierarchy + every object's property values  ───────┘
       └─ ZSTDOBJ "prim id" ──> PRM model chunk ──> objects ──> primitive (vertex+index chunks)
                                                       └─ words[4]>>16 = material id ──> MAT instance
                                                                                          └─ TXID ──> TEX image
LOC  localisation tree (UI/briefing/subtitle text; voiced lines carry stream words)
SND  sound object database  <-names->  WHD/WAV bank  <-streamed data-> pc_eng.str
ANM  skeleton names / animation databases       OCT, RMC, RMI  packed bounding-volume trees; RMI/RMC = room boxes
SGP  room portals (door openings) + routing graph  ZGF, SUP, SGD  constant stubs
```

`bm_formats::loader::SceneFiles::load(&vfs, "Scenes/M01/M01_main")` parses all of them and
`SceneFiles::graph()` returns the joined scene graph.

---

## .GMS - geometry table [H container, M fields]

Module `gms`. Container: `u32 uncompressed_len, u32 file_len, u8 flag(0 = raw DEFLATE), payload`.
Verified for every scene. `Gms::to_bytes(compress)` re-serialises (body identical after decode; DEFLATE
bytes differ from the original encoder).

Decompressed body:

| offset | field | conf |
|---|---|---|
| 0x00 | `u32 table_off` | H |
| 0x04 / 0x08 | 0, 0 | H |
| 0x0c | 4 | ? |
| 0x10 | `u32 stats_off` | H |
| 0x14..0x60 | further offsets/sizes (e.g. `0xa8a80, 0xc4690, 0x2f134` in M01); ROOT object lives at 0x60 | ? |
| `table_off` | `u32 count`, then `count x {u32 offset_word, u32 tag}` | H |
| `stats_off` | `u32 count`, then `count x {u32 type_id, u32 count, i32 unk}` | H layout, ? semantics |

* `addr = (offset_word & 0xffffff) * 4`. Top byte of `offset_word` (0..11) and `tag` (0 or increasing even
  numbers) are **?** (the top byte does *not* equal tree depth).
* The table is the **depth-first** list of all scene geometries (ROOT excluded). The *hierarchy is not in the
  GMS*: the self pointer at +0x04 equals `addr - 0x30` for every entry (it is not a parent pointer). Parent/child
  relations come from the PRP (see below).
* Stats counts are **not** the number of table entries of that type (e.g. M01: 6320 vs 4920 `ZSTDOBJ`); every
  table type id is present in the stats.

Geometry header at `addr` (0x40 bytes, `Gms::geom(i) -> GeomHeader`):

| off | field | conf |
|---|---|---|
| 0x00 | name offset in BUF | H |
| 0x04 | self pointer (`addr - 0x30`) | H |
| 0x08 | pointer to the object's **position block** (3 x f32 + pad), stored *before* the header, usually `addr-0x10` (`Gms::geom_position`); equals the PRP position for 8787/8792 objects of M01 | H |
| 0x0c | slowly growing counter (0x79e, 0x7a4 ...) | ? |
| 0x14 | type id | H |
| 0x1c | control bits (0x07000000, 0x80, 0x03040000 ...) | ? |
| 0x30 | sequence number | ? |
| 0x34 | flags (0, 0x100, 0x200) | ? |

Object spans between consecutive addresses are 0x40, 0x50, 0x70 or 0x80 bytes (the 0x30/0x40 extra bytes
after a header belong to the *next* object's pre-block and are mostly zero). Rotation is **not** reliably in
the GMS; use the PRP.

Common type ids (`scene::type_name`): `0x100021 ZROOM`, `0x100001 ZGROUP`, `0x200002 ZSTDOBJ` (static
model), `0x10003f ZBackdrop`, `0x2000ff ZParticleEmitter`, `0x2000e1 ZBoxPrimitive`, `0x2000ce ZScatterObj`,
`0x800024 ZOMNILIGHT`, `0x800023 ZSPOTLIGHT`, `0x80000d ZENVIRONMENT`, `0x200046 ZWINPIC` (UI picture),
`0x200110 ZSHADOWMESHOBJ`, `0x20001c ZBOUND`, `0x2003e9 ZHM3Actor`, `0x2003e8 ZHitman3`, `0x200012 ZSNDOBJ`,
`0x400003 ZCAMERA`, `0x10044e ZHM3ClipParticleControl`, `0x1000ca ZAllocMany`, ... (M01 has 83 distinct ids).

## .BUF [M]

`"0123456789abcdef"` followed by lookup tables (`u16` pairs, character-class strings), then the geometry
**name pool** (NUL-terminated strings in 16-byte aligned slots). `GeomHeader::name_offset` points at the first
character. `Buf::name_at`. Purpose of the tables: **?**.

## .PRP - property stream ("IOPacked v0.1") [H, byte-exact round trip on all scenes]

Module `prp`. Header: `"IOPacked v0.1\0"`, `u8 is_raw (0)`, `u32 flags (13)`, `u32 unk (0)`,
`u32 key_count`, `u32 table_bytes`, string table (`key_count + 1` NUL-terminated strings: identifiers, enum
names such as `BOUNDING_Static`/`eNormal`, resource paths such as `ROOT\M01/M01_CHARACTERS.zip\...`),
`u32 object_count`, op stream until `EndOfStream`.

Op stream: one opcode byte + payload (table in `prp.rs`): `0x01 Array(u32 n)..0x7c`, `0x02 BeginObject..0x7e
EndObject`, `0x04 Container(u32 n)`, `0x05 Char`, `0x06 Bool`, `0x07/08/09 Int8/16/32`, `0x0a/0b Float32/64`,
`0x0c String` (u32 string-table index when flags bit 3 is set), `0x0d RawData`, `0x0e StringToken`, `0x0f
StringArray`, `0x10 Bitfield`, `0x7d SkipMark`, `0x7f EndOfStream`. `0x03 Reference` and `0x10` and the
"named" `0x80|op` variants are implemented but never occur in shipped files (**M** only).

Structure (verified on all scenes, `Prp::scene()`):

1. Container of **definitions**: `String name, Int32 kind, payload` (kind 2 int array, 3 float array,
   12/14/16 string, 17 string table). Scene-wide values: `CameraFOV 67.4`, `CameraFar 50000`, fog values,
   `StaticShadowSampleHeightLimit`, `PostFilterPalettes`, `ScriptCModule`, ...
2. The scene tree, depth-first. Node = `Object(props)`, `Container(n_controllers)` of
   `String(controller type name) Object(props)`, `Container(n_children)` followed by the child nodes inline.
   Node 0 is ROOT; **node `i+1` = GMS entry `i`** (counts match exactly in every scene; ZGROUP nodes have
   children, ZSTDOBJ never; `ZHM3ClipParticleControl` always has a `TimeOutDelete` controller - all consistent).
3. Trailer `Bool(0)`, `EndOfStream`.

Property objects have **no field names**; meaning depends on the type id (the community
`TypesRegistry.json` of HBM_GMSTool documents ~400 type layouts; not reproduced here). Known common prefix of
every geometry (M): `StringToken(bounding mode)`, `Array[9] f32` (rotation, row-major as stored; unrotated
objects store `0,0,1, 0,1,0, 1,0,0` - the rows are in reversed axis order, determinant -1; the proper rotation is
`R = S^T * J`, see `docs/models.md` *Conventions*), `Array[3] f32` position (identical to the GMS pre-block position).
`ZSTDOBJ` tail `Bool, Int32 prim, Bool`: **prim id = PRM model chunk index** (4712 of the 4920 M01 `ZSTDOBJ`
resolve through the chunk graph; the other 205 have prim id 0 = **no model** (bone/attachment helper nodes such
as `W_Bone_BoltAction_01`). Other node types use the same `Bool, I32 prim, Bool` pattern at `props[3..6]`:
`ZLNKOBJ`, `ZBOUND`, `ZSHADOWMESHOBJ`, `ZScatterObj`, `ZHM3Actor`, `ZHM3ClothBundle` ... - list in `docs/models.md`).

API: `Prp::parse/to_bytes`, `Prp::scene() -> PrpScene`, `Prp::values(range) -> Vec<Value>`,
`scene::SceneGraph::build(&gms, &buf, &prp)` (names, types, hierarchy, `position`, `rotation`,
`stdobj_prim_id()`, controllers).

## .PRM - primitives [H container, M chunk graph, M vertex layouts]

Module `prm`. 16-byte header = chunk 0: `u32 table_off, u32 chunk_count, u32 table_off, u32 0`; chunks are
contiguous, 16-byte aligned; table of `{u32 offset, u32 size, u32 unk1, u32 unk2(0)}` at the end
(`Prm::to_bytes` is byte-exact; `Prm::build` creates new files). `unk1`: 1 for ~90 % of chunks, larger for
shared chunks (reference counts? **?**).

Chunk graph (pointers are chunk **indices**):

```
model chunk (0x40 B)        w[0]=0x00070100 | w[3]=2 | w[4]=parts chunk (shared, 16 B) | w[5]=object_count
                            w[6]=objects chunk | f32 w[8..11]=bbox min | f32 w[11..14]=bbox max
objects chunk               u32 object chunk indices
object chunk (0x40 B)       w[0] = 0x80189/0x80089/0x80181/0x8018b/... (variant flags, ?)
                            w[3]=0x00ff0000 | w[4]=(material_id<<16)|? | w[7]=flags | w[10]=model-ref chunk
model-ref chunk (16 B)      w[0] = primitive descriptor chunk
primitive descriptor (16 B) { vertex_count, vertex_chunk, unk (varies), index_chunk }
index chunk                 u16 1, u16 index_count, u16 indices[] (triangle LIST)
vertex chunk                vertex_count * stride bytes
```

Verified: 4461/4654 M01 primitives (and 99.5 % over all scenes: 81 802 ok, 403 broken chains = scatter/water objects) are triangle lists with `count % 3 == 0` and all indices `< vertex_count`.
`Prm::model()`, `model_chunks()`, `model_objects()`, `prim_vertices()` (raw bytes + stride), `prim_indices()`.

Vertex strides seen across all scenes: 0x28 (61 217 prims), 0x34 (14 984), 0x24 (3 584), 0x10 (2 017). Exact layouts
(verified by `bm-models` tests, details in `docs/models.md`):

| stride | layout (H) |
|---|---|
| 0x28 | `f32 pos[3]`, packed normal +12, color +16, `f32 uv[2]` +20, packed tangent +28, packed binormal +32, pad +36 |
| 0x34 | `f32 pos[3]`, `f32 weight[3]` +12 (4th = 1 - sum), `u8 bone[4]` +24 (**local palette index x 3**), packed normal +28, color +32, `f32 uv[2]` +36, packed tangent +44, packed binormal +48 |
| 0x24 | `f32 pos[3]`, `f32 normal[3]` +12, color +24, `f32 uv[2]` +28 |
| 0x10 | `f32 pos[3]`, color +12 (shadow volumes / collision style meshes; their objects are 0x80 bytes long) |

Packed direction = `[B, G, R, x]` bytes, component = `byte / 127.5 - 1`; color = `[B, G, R, A]` (D3DCOLOR). Stored triangle
winding is clockwise (left-handed D3D). Object variants: 0x40 byte objects (standard), 0x80 byte (0x10-stride shadow
meshes), 0xb0 byte `0xb0101` (water surface grids, no vertex chain), `0x90109` (ZScatterObj procedural scatter, no vertex
chain). Primitive descriptor word 2 = `5 * (triangles - 1)`. Object `w3` bits 16..24 = LOD mask, 24..32 = outfit mask
of skinned meshes (M). Skinned objects reference a bone palette chunk (`(size, offset)` byte pairs, 12 bytes per bone)
and the model's `parts` chunk points at a skeleton block (100 x 64 B bone records with names and parents, world/local
3x4 bind matrices, facial pose bank ...). The "403 broken chains" are the 0x90109 scatter objects and water
surfaces, both now handled.

Coordinates are Glacier units (centimetres) in model space, left-handed Y-up.

## .MAT - materials [H outer layout, M property records]

Module `mat`. Header `{class_table_off, instance_table_off, 0, aux_table_off}`; `u32[32]` class table;
`u32[2048]` instance table (index = material id, ids start at 1; M01: 908 materials); `u32[2048]` aux table
(small values, **?**). Class blocks (`Sprites`, `StaticShadow`, `Standard`, `ScatterMaterial`,
`WaterSurface`, ...) are 16-byte string slots + descriptor records (exposed via `class_table`,
`Mat::class_name` best effort).

Instance: `0x30`-byte header (word1 = class-like number, word7 = `offset-0x10`), NUL-terminated name at +0x30
(e.g. `Locations/M01/Landscape_M01_Jungle_01_Opac`, `Characters/_Nude/Nude_01_TORSO_Tan`), then **16-byte
records** `[tag reversed][u32 value][u32 len][u32 kind]`:

| kind | value |
|---|---|
| 0 | `len==1`: f32 inline; `len>1`: file offset of `len` floats (colors, scale vectors) |
| 1 | file offset of a NUL-terminated string (`len` bytes) |
| 2 | integer inline (`ENAB`, `TXID` texture id, `AREF` ...); `len>1`: offset of `len` words |
| 3 | struct: file offset of `len` further records |

The last `INST` record is the root: `{ NAME, BIND { ...shader properties... } }`; `MatInstance::props()`
returns the `BIND` children, `textures()` the texture slots, `shader()` the `PATH` of special FX materials.
Example (verified): `TEXT { NAME "mapDiffuse", ENAB 1, TXID 155, TILU "TILED", TILV "TILED" }`;
`RSTA { NAME "RenderState", BMOD "TRANS_ON_OPAQUE", OPAC 1.0, ATST 0, AREF 254, CULL "DontCare", ... }`;
`COLO { NAME "v4DiffuseColor", VALU [0.85, 0.83, 0.82, 1.0] }`; `BOOL { BumpEnabled }`; skin materials use
`mapDiffuse/mapNormal/mapSpecularMask`. PRM `object.words[4] >> 16` resolves to an existing material for
100 % of the 4298 M01 objects; ~70 % of them resolve to textures present in the TEX through `TXID`
(the rest use other slot kinds).
Unknown: how the engine picks the `.fx` file (`Shaders/PC/FX/Standard13.fx`, `Skin20.fx` ... per shader
model) for ordinary materials; meaning of `header[1]`, aux table.

## .TEX - textures [H, byte-exact round trip on all scenes, decodes verified visually]

Module `tex`. `u32 table_off (= len - 0x4000), u32 table_off + 0x2000, 0, 4`; entries from 0x10, 16-byte
aligned; 4096-slot offset table at the end. **Orphan** descriptor entries (not in the table) exist.

Image entry: `u32 total_size, [u8;4] fourcc (byte-reversed: "1TXD"=DXT1, "3TXD"=DXT3, "ABGR"=RGBA, "  8I"=I8,
"8V8U"=U8V8, "NLAP"=PALN), [u8;4] fourcc again, u32 id, u16 height, u16 width` (**height first**: verified
visually; the width/height order in older community templates is swapped), `u32 mip_count (1..10), u32
unk18 (0 / 64 for U8V8), f32 unk1c (1.0 / 0.992157 for U8V8), u32 unk20, name\0, mip_count x {u32 len, data}`,
PALN trailer `u32 n + n x 4-byte palette`. Names are not unique (diffuse/normal/specular share one name; the
id is the key). Formats seen: DXT1, DXT3, RGBA (byte order R,G,B,A), U8V8 (normal maps), PALN (UI), I8.
Mips < 4x4 of DXT textures still take one block. `TexImage::decode_rgba(level)`.

Id list entries (`TexEntry::Cube`): `u32 n` (6, 4, 15, 30 seen) + `n` ids; reachable through table slots >= 2048
(also some images, e.g. image 133 at slot 2180); meaning of the lists other than 6 = cube faces: **?**.

## .LOC - localisation [H, byte-exact round trip on all scenes]

Module `loc`. `u8 top_count, u32 top_pos[top_count-1]`, then nodes (`AllLevels` = shared strings, `M01` =
level strings). Node = `key\0 flag`: `0x10` group (`u8 count, u32 pos[count-1]`, children; positions are
relative to the end of the table), `0x09` text leaf (`text\0` + 4 zero bytes), `0x29` voiced leaf (`text\0` +
`u32 a, u32 b` equal pair, e.g. `0xbe130`, related to `pc_eng.str`, unit ?), `0x08` empty leaf. UTF-8, markup
such as `{if shotsfired more 100}`, `<br>`, `<font 2>`, `<color ffffff>`, `{bullet_square}`.
`Loc::get("AllLevels/Actions/ActionKill") == "Action kill"`, `Loc::leaves()`. M01: 3154 leaves.

## .SND [M outer, ? bodies]

16-byte header `(len-16 | len-480 in hideout_main, len, 3, 4)`, stream name `pc_Eng.str`, then sound-engine
objects named `audio:<path>#<kind>` with kind `wave|sound|seq|set|group` (16-byte aligned). `wave` names match the
`.WHD` entries. Bodies (floats such as volume 0.32, flags, links) are **?**. `Snd::names`, `waves()`.

## .ANM [M header, ? payload]

`"MNA\0", u32 size|0x80000000, u32 size`. Small skeleton files (`Loader_Sequence`): `u32 bone_count` at 0x48,
`u32 pose_count` at 0x50, NUL separated names from 0x68 (60 bones `PELVIS, SPINE .. RIGHT_HAND_ATTACHER`, then 55
facial poses `Neutral, A_Mouth_Widen, B_Blink, D_OO:Q:W ...`); header words at 0x30/0x3c/0x44 point just into the
name pool (**?**). Big files (M01: 4 MB) hold per-character *animation database* blocks marked by
`anmcol:animationdatabase#M01_Don` (also `M01_MovieStarAgent`, `SniperRifle_SakoTRG_01`, ...) preceded by an
index list `0,1,2,...,0x268`; the clip headers (64 B each), the bit packed key data pool, the bone id list and the `anim:<Set>/<Name>#<Clip>` names
are decoded by `bm_models::anim::AnmBank` (`docs/anim.md`). `Anm::skeleton`, `Anm::databases`, `bm_models::anim::anm_index`.

## .SGP - room portal graph [H sizes/order, H portals, M edges, ? routing tables]

Module `sgp` (`Sgp::parse`, `Sgp::portals`, byte-exact `to_bytes`, test `sgp_portal_graph_all_scenes`). Earlier notes took this
for the in-game map screen; it is the **door-opening / room adjacency graph** of the level.

```
u32 planes, edges, paths, packed, indices      (header, 20 bytes)
planes x 92 B:  f32 1.0, u32 0, u32 0, u32 room_a, u32 room_b, f32 centre[3], f32 normal[3], f32 corners[4][3]
edges  x 12 B:  u32 plane_a, u32 plane_b, f32 weight
packed x u32    bit packed routing data (**?**)       records[paths] x 3 x u32 (**?**)       indices x u32 (plane / room indices, 0xffffffff = none)
paths:          NUL terminated node path + u32 running index  (rooms like `...\M01_Hacienda_Room_101`, plus map / overlay groups)
```

* **Planes are portals** (H): world space quads in the doorways / windows between two rooms. `room_a < room_b` are indices into
  `paths` (verified: all 1510 planes of the game have both indices < path count and `room_a < room_b`; every connection is
  stored once). M01 portal 0 = `(3900, 305, -3300)`, the hacienda front door between `M01_Outside_01a` and a ground floor room
  (the "z = -3300" of the older notes is just the facade plane). The boxes of `.RMI` on both sides of a portal belong to its two rooms
  (70 %+ of M01 portals, the boxes are loose) and > 50 % of M01's doors stand within 2.2 m of a portal (`bm-game` test).
* **Edges** (M, 95 %+ exact): connect the portals that touch a common room, `weight` = euclidean distance between the portal
  centres: the intra-room routing graph for room-level path planning (the original probably plans room by room, then
  portal by portal).
* `packed` (15 671 words in M01), `records` (3 words per path, `0x803a0015`-like `u16, u16` pairs with flag bits) and `indices`
  look like precomputed routing / next-hop tables and per-room portal lists: **?** (bm-game derives adjacency from the planes instead).
* The path table contains one entry per room (matched to `ZROOM` by the last path segment) and other nodes (scope overlays,
  map groups, backdrops) whose records are zero.

## .RMC / .RMI - room volume maps [H layout + records, M semantics, ? tree nodes]

Module `roommap` (`RoomMap::parse`, byte-exact `to_bytes`, tests `roommaps_all_scenes`). Community names (ReHitman / BMEdit notes):
**RMC = Room Collision tree, RMI = Room Inside tree**; both files of a scene have the same grid and record layout. A packed
bounding-volume tree over a list of *room boxes*:

```
u32  leaf_off           byte offset of the leaf list (H)
f32  origin[3], f32 scale   quantisation grid: world = origin + (q - 32768) / scale, q = u16 (H, verified on all 28 files)
[u16;3] x N             tree nodes (6 bytes), 0xCD fill up to leaf_off
leaf list, 16 B each:   u32 record_offset, u16 min[3], u16 max[3]      (quantised AABB, rounded outwards for ~97 %)
records, 84 B each:     u32 axis_aligned, f32 rot[9], f32 centre[3], f32 pivot[3], f32 half[3], u32 id, u32 link
```

* `leaf_off` and the record count: records follow the leaf list directly; the leaf list is as long as the record list
  (M01 RMC 96 / RMI 107, M13 17 / 39, hideout 4 / 4). Leaves point at records by byte offset in a *different order* than the
  records are stored.
* **Record** (H layout, M meaning). `centre` is the world centre of the box, `half` its half extents. `rot` uses the stored
  reversed-axis convention of `docs/models.md` (an unrotated box stores `0,0,1, 0,1,0, 1,0,0`). `axis_aligned == 1` means the
  exporter already expressed `half` along the *world* axes (quarter turns of the source zone are baked in and `rot` only
  documents them); `0` = arbitrary angle, `half` is along the local axes (world axis of local axis `j` = stored row `2 - j`).
  Verified by the leaf boxes: with this rule 98 % of all leaf boxes enclose the record box (the few exceptions are
  tighter than the nominal box, mostly `.RMC`). `pivot` = origin of the `InsideBound` `ZBOUND` node the box was exported from (0 for boxes
  that do not come from one), so `centre - pivot` is the model bbox centre offset.
* **RMI boxes are the room volumes.** The `InsideBound*` `ZBOUND` nodes (`0x20001c`, props `[.., I32 prim, Bool]`, size =
  PRM model bbox, centred on the node + bbox centre) below a `ZROOM` appear 1:1 as RMI records (77 of 107 in M01, matched by
  world centre < 2 cm and volume). The other records (30 in M01) are the volumes of rooms that have no `InsideBound` node.
  RMC holds similar boxes grown to cover floors / ceilings (collision of the room shell); `.RMC` is not used by bm-game.
* **Record `id` = room.** All boxes of one room share an `id`; the distinct ids in ascending order correspond one to one
  to the `ZROOM` nodes (`0x100021`) whose property 11 (`I32`, number of volumes) is positive, in scene node order. Verified: 12
  of 14 levels have exactly as many ids as such rooms (M01 48/48, M06 77/77, M10 129/129, hideout 3/3 ...) and every
  `InsideBound`-matched box agrees with the rule; M02 and M11 have one extra id each (assignment falls back to the matched
  boxes). The ids are *not* node indices or GMS sequence numbers. One degenerate record (zero size, id 0xdd / 0x100 / 0x45a) is
  the "no room" placeholder. `link` (`u32`, mostly 0, sometimes a value like 0xada) is **?**.
* **Tree nodes** (`[u16; 3]`): bit 15 of the first word marks leaf style nodes, the low bits and the other words look like
  child / leaf indices and axis codes of a kd-style subdivision (`0x8003`..`0x800f` are common). **?** - not needed because a
  linear scan over ~100 leaves is cheap; `RoomMap::records_at(p)` / `smallest_at(p)` do it (leaf box test, then the record
  box test).
* Nesting: room volumes overlap (a small indoor room inside the big outdoor box), so the room of a point is the *smallest*
  containing box (`bm_game::rooms::RoomIndex`).

## .OCT - "physics octree" [H container, M leaf boxes, ? leaf ids]

Module `oct` (`Oct::parse`, byte exact). Not an octree of triangles: a **list of small packed bounding-volume trees** that
use the same tree / leaf encoding as RMC/RMI, followed by an id table:

```
u32 1, 1, 1, 1                            file header
blocks (16 B aligned, zero fill between them):
  kind A (149 in M01, scales 0.08 .. 32): u32 size, f32 scale, f32 origin[3], u32 0,0,0     (outer header, 0x20 B)
  kind B (1166 in M01, scale 3200, units ~m): u32 size, u32 0x00ff4f1c, f32 origin[3], f32 scale, u32 stale_ptr, u32 0
  inner header:  u32 leaf_off (relative to the inner header), f32 origin[3], f32 scale       (as RMC/RMI)
  [u16;3] tree nodes, 0xCD fill, then leaves  { u32 id, u16 min[3], u16 max[3] }
  block end: A = block start + size + 0x20, B = block start + size
tail: u32 0xffffffff, u32 0, then (u32 a, u32 b) pairs (5365 in M01: ids with offsets in steps of 0x20)
```

* The first four A blocks (scale 0.08, origin = scene bounds) cover the whole level; later A blocks have the origin of a
  room box (`(-1675, 4850, -2200)` = M01 `YardSmall` `InsideBound_01`) - one or three blocks per room. B blocks are per object
  trees in object space. All 68 scenes parse completely (`oct_blocks_all_scenes`); 100 / 180 byte files are empty / two-leaf
  placeholders.
* The 16-bit quantisation (leaf boxes ~110 cm cubes in A blocks) is decoded; the **leaf ids are not**. They are < node count
  (5417 distinct in M01, range 2..8658) but do not match node indices, GMS words or sequence numbers (tested: the leaf box
  encloses the node's world bounds for only 4 % of the ids). Collision geometry therefore still comes from the PRM meshes
  (`WC_*` collision proxies, see `docs/game.md`); `.OCT` is exposed for tooling only.

## Gameplay nodes (property layouts) [H structure, M semantics]

Module `gameplay` (typed decoders over `SceneGraph`; counts verified on all `*_main` scenes by `gameplay_nodes_all_scenes`).
References are `ROOT\a/b.zip\Node!Path` strings; `PathIndex` normalises them (`/`, no `ROOT/`, lower case) and resolves them to
node indices (`PathIndex::resolve`).

| node / controller | layout | meaning |
|---|---|---|
| `SingleDoor` / `MultiDoor` controller (26 / 21 in M01) | `[Token state (IDS_CLOSED/IDS_LOCKED/IDS_OPEN), Bool x6, Token AWAY, I32 snd_open, I32 snd_close, Str "", Token lock (IMPOSSIBLE / EASY ..), Token OPEN/NOTHING, I32 x4, Token SERIAL/PARALLEL, Token SERIAL, Token DT_NORMAL/DT_MATPOSANIM, F32 speed (1.0 / 0.8), Token PUPGRADEDAMMO]` | initial state (locked doors have lock-pick difficulty `EASY`), swing away from the opener, `.SND` offsets of the open / close sounds, sliding doors use `DT_MATPOSANIM` |
| door geometry | single door: the node itself (a ZSTDOBJ whose model spans local x = -95..5, origin = hinge); multi door: a group whose children `D1`, `D2` are the leaves (mirrored leaves span x = -5..95 at x = +-95); sliding doors (`DDoor_Sliding*`, `DT_MATPOSANIM`): leaves at +-33 on the group's width axis | hinge at the leaf origin, swing about local Y; no open angle is stored (bm-game uses 90 deg, away from the opener); slide direction/distance are derived from the leaf position and bounds (`MatPosAnim` controllers carry `[Str "Activate", Array(0,0,0), Bool x9, F32 50, F32 25, I32 -1, I32 -1, Token]`, the actual motion is not stored in the scene) |
| `ZHM3Actor` (0x2003e9) props | `[.., Bool, I32 prim, Bool, I32 outfit, I32 .., Str "anmcol:animationdatabase#M01_Don", I32, I32, I32, Container, Str, I32, Str]` | animation database, outfit (`docs/anim.md`) |
| `PathFollower` controller (51 in M01) | `[Container<Str ZLIST refs>, Token LOOPING/RANDOM, Bool x3]` | patrol: walk the lists of waypoints in order (`LOOPING`) or pick random ones (`RANDOM`) |
| `ZLIST` (0x800001a) | props[5] = `Container<Str node refs>` | waypoint lists, help lists (`Controllers!Helppoints!..`), exit lists (`ExitAreas!00_ExitList`), elevator target lists |
| `ScriptC` controller (212 in M01) | `[Str module ("m01\m01_donchilean", "alllevels/guard", "m01/m01_levelcontrol"), params ... , SkipMark]` | binds a *native* script (compiled into `Scriptcs/_gamerelease/<level>.dll`) to the node; the parameter arrays are module specific. Useful string params: node refs, weapon item refs (type `0x100429` nodes such as `Gun_SnubNosed_01`, `SMG_FamaeSAF_01`), `Helplist` lists. The level controller's script (`<level>_levelcontrol`) lists the target actors in objective order |
| `ZBoxPrimitive` (0x2000e1) | `[base.., Bool, I32 0, Bool, F32 x, F32 y, F32 z]` | trigger box, full size x/y/z centred on the node origin |
| `Action` controller on a box | `[Str label ("//alllevels/actions/escape"), Bool, Token AT_GENERIC, Str event ("M01_ExitPlane"), Str target ref, I32, I32, Bool, Str "ActionHide", Str "ActionShow", Str]` | exit areas (`Controllers!ExitAreas!PlaneExit` 200 cm box, listed by `00_ExitList`) and mission actions |
| `Action` on an item node (66 in M01) | `[Str "_Pickup", Bool, Token AT_PICKUP, Str, Str, I32 range 200, I32, Bool, ..]`, node props `[.., Str ref to the item definition, I32, I32 amount]` (types 0x100434 ammo, 0x10042a items, 0x10042b close-combat weapons) | world pickups |
| `CheckInside` controller on a box | `[Str watched ref (Hero Ground), Str target ref, Str enter msg, Str leave msg, I32/Bool x5]` | trigger volume notifying a script |
| `SecurityZone` on a box | `[Bool, Token eZone1_PATCH]` | trespassing zone box |
| `ZoneControl` on a box | `[Token eZone2_PATCH, Container<Str room-group refs>]` | zone -> rooms table (refs name `ZROOM` nodes or whole location groups) |
| `ZHM3HitmanMovementGuide` (0x200435) | `[base.., I32 prim, Bool, Token eClimbBox, Array<12 f32>]` | climb boxes (4 corners in node space) |
| `Usepoint*` (0x200445) | `[.., F32, Str]` + `ScriptC` (`m01/m01_useable_getautograph`) | scripted interaction points (139 in M01); not used |
| `MusicLocationDef` / `SoundLimiterFade` | see `docs/sound.md` | room lists for music zones / audibility |
| `ZROOM` (0x100021) props | `[.., Bool, Bool, Bool, Bool, Bool, I32 volumes (11), I8 (12), I32 (13), Token eNormal, Token eINSIDE / eOUTSIDE / eUNDEFINED, Container<[Raw portal data, Bool, Bool, I32 a, I32 b, Str ambient, Array]>, ..]` | room flags; the `Raw` payload (`u32 n, u32 0`, `n` x 32-byte records of 6 floats + 2 words, then 92-byte records `u16 room_a, u16 room_b, plane data ...` - the same stride as the `.SGP` portal planes) repeats the room's sub-boxes and portals (**?** beyond that) |

**Objectives (LOC).** `<Level>/MissionObjectives/<n>/{Objective, ObjectiveDetail, Type, WhoOrWhat}` (e.g. `M01/MissionObjectives/1/Objective =
"Kill Don Fernando Delgado"`); `Type`: `T` target, `E` escape, `R` retrieve, `P` protect / smuggle, `O` optional. Named keys
(`RehabTarget1` ..., M04) and `Temp` placeholders also exist. `gameplay::mission_objectives` returns them in order.
The *logic* of the original missions (kill conditions, scoring, notoriety, disguises, cut scenes) is native code in
`Scriptcs/_gamerelease/*.dll` and is **not decoded**.

## .ZGF / .SUP / .SGD [H, constants]

17 / 24 / 8 byte constant stubs in every one of the 68 scenes (`misc::ZGF_BYTES`, `SUP_BYTES`, `SGD_EMPTY`; the real-file test asserts
byte equality). They carry no data: `ZGF` and `SUP` are Glacier type tags (`TFGZ` / `DLCF` reversed, with the sizes 17 / 24 in their
header), `SGD` = `ffffffff cdcdcdcd` is an *empty* marker (uninitialised MSVC fill). Their loaders exist in the engine for
scenes that ship content (a ZGF is probably a zone / geometry file, SUP a support / script table); none of the shipped scenes does.

## Non-binary scene members

`Shaders/PC/FX/*.fx` (DirectX 9 effects), `menu/*.xml` (UI), `*.dat`: plain text/XML (not parsed by this crate).

## Survey tools (agent #14)

`cargo run --release -p bm-formats --example <name> -- Scenes/M01/M01_main ...`: `nodes <type-hex|text> [n]` (props + controllers of nodes), `nodeinfo <0xtype|ctl:Name|name:text>` (compact
dump), `rooms` (ZROOM props + GMS words), `bounds` (ZBOUND + PRM bbox), `doors`, `actors`, `ctlsurvey` (controller histogram), `levelctl` (level controller script +
LOC objectives), `locdump <text>`, `gp` (decoded gameplay summary), `sgp` (portal graph), `subtree <node>`, `bbox <nodes...>`, `near x y z [r]`, `raw <node>` (Raw payload hexdump);
`bm-game --example room_probe` (room map coverage, point lookup) and `oct_probe` (OCT leaf id hypothesis test, negative result).

## Open questions (for later agents)

* GMS: meaning of the unknown header words (0x14..0x60), table top byte/tag, flags at +0x1c/+0x34, per-type
  extension blocks, stats counts.
* PRP: field names per type (needs a type DB), `Reference`/`Bitfield` ops (never seen).
* PRM: `unk1`, meaning of the object words `w0`/`w6`/`w7`/`w11` bits and of the model header `w3`, ZScatterObj cell
  tables (layouts, skinning, descriptor word 2 and the palette scheme are solved - `docs/models.md`).
* MAT: class->shader mapping, `header[1]`, aux table, `COLO/FLTV` details beyond the examples.
* TEX: `unk18/unk1c/unk20`, slot >= 2048 rule, non-6 id lists.
* OCT: leaf ids and the tail pair table; RMC/RMI: tree node semantics, record `link`, the portal / room graph inside the `ZROOM` `Raw` payload; ANM is decoded (`docs/anim.md`). SND: object bodies. SGP: `packed`/`records` arrays.
* Gameplay: the native scripts (`Scriptcs/*.dll`) hold the mission / AI logic and the meaning of most `ScriptC` parameter arrays; door animation curves (`MatPosAnim`), `Usepoint` behaviours, `ZHM3HmAs` AI tables.
* LOC: unit of the voiced-line stream words.
