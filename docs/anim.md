# Animation (`.ANM`) and character outfits

Everything here is implemented in `bm-models` (`anm_decode`, `rig`, `anim`, `scene::actor_outfit`, `mesh::resolve_outfit`)
and consumed by `bm-game::SceneClipProvider`. Numbers were measured on the user's own game files; the tests in
`bm-models/tests/real_files.rs` and `bm-game/tests/real_levels.rs` (skipped without the game directory) assert them.
Confidence legend as in `formats.md`: **H** asserted by a test on all shipped data, **M** consistent with samples,
**?** unknown.

## 1. Where the clips are **H**

A big level `.ANM` (M01: 4.25 MB, 887 clips; the 26 scene banks together hold 12 963) ends with three consecutive tables that were the
missing piece of the earlier attempts (the `(id, offset)` tables around `0x28800` are *not* the clip data, they are the
animation-set / database bookkeeping and are not needed to play clips):

```text
...  clip headers      N x 64 bytes                     N = number of clip names (887 in M01)
     key data pool     bit packed streams, starts directly after the last header
     bone name list    "PELVIS\0SPINE\0SPINE_1\0 ... GROUND\0 camera_bone\0 ... w_bone_...\0"   (204 ids in M01)
     clip names        N x "anim:<Set>/<File>#<Clip>\0"  or "extra\0" for alias entries without a name
     facial pose names "Neutral", "A_Mouth_Widen", ... (67 in M01)
```

`AnmBank::parse` anchors on the bone name list (`PELVIS\0SPINE\0`), counts the clip names, and finds the headers as the
last `N` records of the longest run of plausible 64-byte records in front of it. Clip header (`u32` words):

| word | meaning |
|---|---|
| 1 | `fps << 16 \| frame_count` (always 25 fps in shipped data) |
| 3 | size of the clip's data in the pool (bytes) |
| 4, 5, 6, 8 | pool offsets of stream **A**, **B**, **C**, **D** (`0xffffffff` = absent) |
| 7 | multiple of 16 (< 0x40000), purpose unknown |

Clip names carry the set (`Hero`, `Male_Reg`, `Female_Reg`, `Weapons`, `Cutscenes/...`), a file and the clip name:
`anim:Male_Reg/Male_Reg_Walk#Walk` (29 frames), `anim:Hero/Hero_Run#Run_Forward`, `anim:Male_Reg/Male_Reg#Base_Pose`.
`AnmBank::find(name)` / `find_suffix` / `search` look them up (case-insensitive, `anim:` prefix optional).

## 2. The stream format **H**

A clip has up to four streams; each is a list of *channels* over all frames:

* **A** - the 74 "rig parameters" of the humanoid (10 quaternions + 64 scalars, see section 3),
* **B** - extra bones as quaternion + translation (`RIGHT_HAND_ATTACHER`, `LEFT_HAND_ATTACHER`, `FREE_BONE01`,
  `camera_bone`, weapon parts `w_bone_*`), the ids index the bone name list,
* **C** - root (`GROUND`) motion: quaternion + translation,
* **D** - rarely present extra scalars (unused so far).

Stream layout (`anm_decode::decode_stream`):

```text
u8  n                       channel count
u16 id[n]                   channel ids (rig parameter index for stream A, bone name id for B/C/D)
u8  desc[n]                 low nibble = channel type, high nibble = bits per quantised value
... padded to 16 bytes (call this position p)
u16 at p+2 (w2), u16 at p+4 (w4)
static section  at bit p*8 + 0x30: per channel the base value / range and "constant over the clip" flags
u16 block_table[blocks+1]   at bit (static start + w2*16): offsets (in 16 bit units) of the three sub streams of each block;
                            blocks = ceil(frames / 8)
per block, three bit streams: A = first sample residuals, B = last sample residuals, C = (bit width, signed deltas) for the
                            frames in between (C starts at `(t0*2 + w4) * 8` bits after the data base)
```

* Channel types: 0 angle (-pi..pi), 1 unit scalar (-1..1), 2 scalar with an explicit 16 bit range, 3 quaternion,
  4..8 quaternion + 3 translation scalars with range +-10, 100, 1000, 10 000, 100 000.
* Frames are stored in **blocks of 8**: every channel stores, per block, the first and last sample explicitly and the
  seven samples between them as signed deltas of a per channel/block bit width (4 bit width field, 0 = all equal), with
  a "constant over the block" shortcut flag. 8 frame blocks overlap at the boundaries (9 samples per block).
* Quaternions use **smallest three**: a 3 bit selector per sample (largest component index + sign) and three quantised
  components, reconstructed as `component * 1.42 / 2^bits - 0.71`; the fourth component follows from unit length.
* All bit fields are little endian, read through absolute bit positions.

Validation: all 12 963 clips of the 26 scene banks decode without error, every decoded quaternion has unit length
(`anm_bank_finds_every_clip_and_decodes_it`, `anm_banks_of_every_level_decode_and_animate_their_actors`).

## 3. Rig parameters -> bone transforms **H** (limbs: IK, `rig.rs`)

Stream A does not store local bone rotations. It stores the controls of the engine's humanoid rig; `RigSolver::solve` turns
them into local transforms of a PRM skeleton (table in `rig.rs`):

* `q0` pelvis rotation + `s10..12` pelvis position in leg lengths, `q1` spine twist shared by `SPINE/_1/_2` + `s13..15` the
  position of `SPINE_2`, `q2/q3` neck / head, `q4/q5` feet, `q7/q9` clavicles, `q6/q8` hands;
* ankles (`s16..18`, `s21..23`) and wrists (`s26..28`, `s30..32`) are **positions** solved by two bone IK (law of cosines
  with a swivel angle for the knee / elbow plane), fingers are euler triples + a flex angle per finger;
* the face bones and the `xtra_*` helper muscles are not driven (they stay in the bind pose).

Retargeting: the rig works in leg-length units, so one clip drives all skeletons (the shipped female skeleton has 82.8 cm
legs, the males 87.8 cm; the player skeleton is a 100-bone model, NPCs have 96/97 bones - all have the humanoid bones the
solver needs; 98 of the 106 distinct actor skeletons of all levels play the walk clip, the rest are props / animals).

Partial clips (upper-body layers `UB_*`, hit reactions) leave parameters unanimated; `AnmBank::raw_clip` reports a mask
and `animation_with` fills the rest from the `Base_Pose` clip.

Sanity checks that pass (`walk_clip_is_an_anatomically_sane_loop`, `base_pose_is_close_to_the_bind_pose`,
`every_main_clip_builds_an_animation_for_the_player_skeleton`):

* limb bone lengths stay within 2 % of the bind pose over the whole walk, the head stays 150-190 cm above the floor,
  feet 0-40 cm, the pelvis 80-110 cm;
* the IK reaches its targets: the left ankle sits within 0.6 cm of parameters 16..18 in every frame;
* consecutive frames never move a bone more than 25 cm, the loop closes (first == last frame within 3 cm), the feet swing
  over > 50 cm and the root travels 100-200 cm/s along -Z (`Animation::root_speed`);
* `Base_Pose` is close to the skeleton's bind rotations (< 0.35 per bone) and puts the pelvis at the model height.

## 4. Using it

```rust
let scene = bm_models::Scene::load(&vfs, "Scenes/M01/M01_main")?;
let bank = scene.anim_bank().unwrap();                             // AnmBank, parsed once
let i = bank.find("Male_Reg/Male_Reg_Walk#Walk").unwrap();
let anim: Animation = bank.animation(i, &skeleton).unwrap();       // tracks per bone, looping flag, root_motion
let pose = anim.sample(&skeleton, t);                              // -> skeleton.skinning_matrices(&pose)
```

Examples: `anm_clips [scene] [filter] [detail]` lists the clip table, `anm_probe` shows per skeleton solver / leg length / head
height, `actor_probe`, `outfit_survey`, `mesh_probe`, `mat_list` are the outfit and material research tools.

`bm-game` plugs this in as `SceneClipProvider` (`Game::load` installs it, with the procedural provider as fallback):

| state | player (`Hero`) | male NPC (`Male_Reg`) | female NPC |
|---|---|---|---|
| idle | `Hero#Stand_Do_Nothing` | `Male_Reg#Stand_Relaxed` | `Female_Reg#Stand_Tense_01` |
| walk | `Hero_Walk#Walk_Forward` | `Male_Reg_Walk#Walk` | `Female_Fat#Walk_Forward` |
| run | `Hero_Run#Run_Forward` | `Male_Reg_Run#Run` | `Female_Fat#Run_Forward_Fat` |
| crouch / crouch walk | `Hero_Crouch#Crouch_Ambient` / `Crouch_Forward` | procedural | procedural |
| hit | `Hero#HitShoulderRightFront` | `Male_Reg_Hit#HitTorsoFrontBig` | same as male |
| dead | `Male_Reg_Die#Die_Backward` | `Male_Reg_Die#Die_Backward` | `Female_Fat#Die_Backwards` |

Locomotion clips are driven by the gait phase of the simulation, scaled by the clip's own root motion
(`phase * cycle_cm / clip_stride`) so feet do not slide; one-shot clips play once and hold the last pose. Female skeletons
are recognised by their leg length (< 85 cm). `Aim/Fire` use the procedural pose for the player (the weapon
holding layers `UB_*` are not blended yet).

### Facing, weapon holds and held models

* Characters face **-Z** of their model (bind pose toes, walk clip root motion). The simulation heading is `forward(yaw) = +Z`,
  so spawn yaws come from `math::character_yaw` (heading of the node's -Z) and `render_state` draws characters with
  `math::character_render` (pose x half a turn). `bm-models/examples/facing_probe.rs` prints the evidence.
* The player's drawn weapon selects an upper body layer: lowered/carried `Hero_UB#UB_Hold_Gun_Side` (pistol), `UB_Hold_SMG`,
  `UB_Hold_Rifle_Special`; while aiming the per weapon `WeaponDef::hold` clip of the `Hero_1Person` set (`UB_1P_Hold_USP`,
  `Ub_1P_Hold_MP5`, ...). `SceneClipProvider` overwrites only the bones below `SPINE` with the first frame of that clip (legs
  keep walking; for one handed holds the free left arm keeps the locomotion clip) and tilts the chest by the camera pitch
  (`AnimRequest::aim_pitch`). Armed NPCs aim with `Male_Reg#Shoot_Pose_Gun01` (last frame held).
* Shots: `World::aim_dir` casts the crosshair ray from the *camera* (shoulder camera) and fires from the eye at that point.
* Weapon models are the `Weapons/<class>/<name>/Ground01` links of the scene (`bm-game/src/props.rs`, `WeaponDef::model`).
  A held weapon is drawn at `hand attacher * grip * (weapon group -> model link)`; the grip is derived from the aimed pistol
  pose so that the barrel points along the character front (`AnimationProvider::weapon_grip`). Ground pickups use the item
  model named by the pickup node (`CC_Hammer_01`, `SMG_FamaeSAF_01` ...). `bm-app` creates one extra scene item per
  (owner, weapon) (`Game::prop_slots`) and `RenderState::props` says which are visible.
* `bm screenshot --aim` holds the aim button, handy to check the poses. `bm-models/examples/weapon_probe.rs` lists weapon
  models, their bounds and muzzle markers.

## 5. Open items

* Facial animation: decoded and applied since agent #13 (section 7). Open: no shipped clip animates `FA_*` bones, so expressions
  (`C_Smile`, `C_Pain_*`) are only driven by code; the meaning of `LIP` channel 0; eye-look tracking.
* Stream D, header word 7, the per block selector semantics beyond what is needed for exact reconstruction.
* Clips of non-humanoid sets (weapons: B/C streams only, cut-scene sets with their own skeletons) decode, but only the
  extra bones that the target skeleton names are animated; weapon / attacher bones are not attached to the hands yet.
* Layering (upper body clips over locomotion), blending between states (`AnimState` changes snap), turn / start / stop clips,
  ragdoll, `Dying_Recovering`, sitting / use-point clips (the original NPC scripts decide when they play).
* The root motion of a clip is exposed (`Animation::root_motion`) but the game moves bodies itself.

## 6. Outfits and faces **H**/**M**

One combined skinned model per character class holds *every* outfit and LOD of that class (the player model has 191 skinned
objects, the M06 party guest 648). Object word 3 (`Mesh::group`) selects:

* bits 16..24 - LOD bit mask (`0x01` = most detailed, `0x02` next, `0x7c` the distant ones, `0x80` an extra set), `Mesh::is_lod0`;
* bits 24..32 - **outfit value** (`Mesh::variant_mask`): low two bits `1..3` pick the base kind (hair colour / skin variant),
  the upper bits pick the clothes (`1, 5, 9, 13, 17, 21, ...` for kind 1; `3, 7, 11, 15` for kind 3).

A `ZHM3Actor` stores the outfit value it wears as its **property 6** (`I32`; for example `1`, `5`, `9`, `13`, `17`, `37`)
and the animation database as property 8 (`anmcol:animationdatabase#M01_Don`). The renderer draws a skinned mesh iff
`variant_mask == 0 || variant_mask == actor_outfit` (`Mesh::in_outfit`) at LOD0; actors without a matching mesh set fall
back to the lowest "normal" outfit (`Model::resolve_outfit`). `Scene::actor_outfit(node)` / `actor_database(node)` expose
the properties; `RenderEntity::outfit` carries the resolved value to the renderer (`UploadOptions::actor_outfits`,
`outfit_by_node` for disguises).

Checks: every one of the 71 M01 actors draws a complete body (feet to head) with at most one head material, the Don and the
film star share model 4087 but wear their own suit and face (outfit 5 -> `DonChilean_01_*`, outfit 1 -> `MovieStar` ...), the
`HMAS_*` cloth bundle links (`ZLNKOBJ` under `Clothbundles!...`, property `3/7/15`) are the same outfit values for disguises.
Verified visually with `bm screenshot --focus <name>` on M01 and M06 characters.

The facial skin is a separate small mesh per outfit (820 / 154 / 294 triangles) and the hair another; their material
references only make sense with the **material reference bias** below.

### Material references are instance id + 1 **H**

`words[4] >> 16` of a PRM object is the `.MAT` instance id **plus one** (0 = no material). Evidence: the shadow meshes of
M03 resolve to shadow materials 62/62 with the bias and 0/62 without; the last instance of M01 (id 908) can only be
addressed with it; node names match material names three times more often (`Glass_OFF` nodes use `Glaass_..._OFF`);
heads, hair and eyes of the characters line up with their geometry (head 820 tri, hair 154/294 tri). Before this fix every
static surface of the game showed the *previous* material in the `.MAT` list; characters had hair textures on their faces.
`decode::material_of`, `ModelObject::material_instance`, the PRM writer and the tests use it.

### Related renderer facts found on the way

* `U8V8` normal maps are *unsigned biased* (flat = 127/128), not signed: decoding them as signed made every normal-mapped
  surface a field of stripes (`bm-formats::tex`).
* `.MAT` instances 1..5 are unnamed untextured placeholders (green boxes around furniture); the renderer skips them like the
  `_Glacier/*` helper materials.

## 7. Facial poses **H**/**M** (agent #13)

Each skinned model's skeleton block holds its own **face pose bank** (`parts[1]` list entry 11 -> `[records, offsets, record_count,
map, slot_count]`; `Skeleton::face`, `Skeleton::apply_face`):

```text
records  record_count x 44 B:  f32 axis[3], f32 angle (rad), f32 translation[3], f32 scale[3], u32 bone index   (a DELTA on the bind pose)
offsets  u32[slot_count+1]:    slot s owns records offsets[s] .. offsets[s+1]
map      u16[128]:             pose NAME index (order of the .ANM facial pose list: 0 Pose_DeformTest, 1 Neutral, 2 A_Mouth_Widen, ...) -> slot, 0xffff = not in this bank
```

* Banks differ per model: the male banks have 55 poses, the female bank only 30 slots (no `D_S:Z:C`, `D_G_R`, `C_Smile_C`, ...).
  `Neutral` is an empty slot (the bind pose *is* neutral); in the female bank `Pose_DeformTest` is slot 0 with 17 records.
* Evidence: `A_Mouth_Open` moves `FA_JAW` (0.14 rad about the jaw's X axis), `B_Look_Left/Up` rotate the eyes about Y / X, `B_Blink` closes
  both eyelids, `A_Mouth_Widen` moves the mouth corners outwards in the head frame; `D_Closed` only touches lip bones
  (test `facial_pose_banks_match_the_pose_names`).
* Frames: rotation is axis/angle applied in the **bone's own frame** (`R = R_bind * R_delta`; the jaw hinge works only that way), the
  translation is **added to the bind translation in the parent frame** (the mouth corner deltas only make sense in the rotated HEAD frame),
  scale multiplies. Poses blend additively with weights (`apply_face(&mut pose, &[(pose_index, weight)])`).
* The pose names live in the `.ANM` (`AnmBank::facial_poses`); `Skeleton::facial_poses` is filled by `bm-game` (`skeleton_for`).
* **Lip sync**: the channel ids of the `LIP ` chunks (`bm-sound::LipSync`; 0, 14..24, 40..43) are exactly these pose indices
  (`D_F:V`=14, `D_A:I`=15, `D_OO:Q:W`=16, ... `D_E`=24, `D__Jaw_Open`=40, `D__Jaw_Frw`=41, `D_D_L_N_T`=42, `D_TH`=43); the 0..255 key weights are
  pose weights. `bm_game::face::FaceController` (per entity, applied in `Game::render_state`): rest = `D_Closed` x1 (closes the slightly parted
  lips of the bind pose), `B_Blink` every 2.5-6 s, `Game::speak(entity, Arc<LipSync>)` drives the visemes and relaxes `D_Closed`.
  Channel 0 is ignored (meaning unknown). Nothing in the game calls `speak` yet (NPC dialogue playback is not implemented).
* Debug: `bm screenshot --focus <name> --focus-dist=-45 --focus-height 160 --face "A_Mouth_Open=1,C_Smile=0.5"` (`--face none` = bind pose;
  a negative `--focus-dist` views from the other side - the default side shows the back of most characters).
* The red lips are the lipstick of the face texture; the red inner mouth seen before came from the parted bind-pose lips and is gone with
  `D_Closed`. No material fix was needed.
