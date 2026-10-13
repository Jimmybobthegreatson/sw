# Particles (bm-particles)

Confidence: **H** verified against the files (values behave as named in the sim/render output), **M** consistent with
data + naming but not proven, **L** guess / default.

## Where particles live in the data (H)

No dedicated particle file exists: no `.fx` or other particle format in the scene ZIPs, and no `Shaders/` folder in
the install. Particles are scene-graph nodes (GMS + PRP, joined by `SceneFiles::graph()`):

| type id | name | count (all scenes) | role |
|---|---|---|---|
| `0x2000e5` | ZParticleTemplate | 0xa36 | behaviour + sprite of one particle kind; 36 props, `*_main` scenes hold ~100 |
| `0x2000ff` | ZParticleEmitter | 0x23b0 | placement + spawn schedule; 32 props; `props[31]` = template node *name* |
| `0x2000e4` | ZParticleController | 0x1f | empty node, one per scene (global manager) |
| `0x10044e` | ZHM3ClipParticleControl | 0x19c8 | weapon clip/shell effect trigger, not an emitter (ignored) |

Template/emitter definitions are duplicated per scene (premission scenes carry the same ~94 templates as main); lookup is by
node name. Bullet impact effects (`Effects/Bullet_Impact.zip/<Material>_01/Emitter_*`) are emitters that are not
auto-started and are activated by messages (`props[25]`, `"Activate"`) from game logic. Ambient effects (fire, steam,
shower, lamp sparks, electric glows) have `bAutoStart`.

## ZParticleTemplate props (36)

| idx | meaning | conf |
|---|---|---|
| 4 | PRM chunk id of the sprite: 5 words `{0x20000, texture id, 0, 7, blend}` -> `.TEX`; blend `0x4c001` = alpha, `0x4c002` = additive | H |
| 6 | constant acceleration vec3 (cm/s^2); gravity is `(0,-1000,0)` for sparks/debris, smoke rises `(0,+20..50,0)` | H |
| 7 | velocity jitter / turbulence (cm/s) | M |
| 8, 9 | lifetime (s), lifetime variation 0..1 | H / M |
| 10, 11 | start size (cm), size variation 0..1 | H / M |
| 12 | size growth (cm/s) (smoke 15..40, flash 140) | H |
| 14 | spin (deg/s) | M |
| 17 | bool: random initial rotation | M |
| 21 | velocity stretch factor (>0 means streak, e.g. sparks `0.015`) | M |
| 13, 15, 16, 24, 25, 26 | unknown (kept raw in `TemplateRaw`) | L |
| 35 | array of 8 RGBA keys (0..255), evenly spaced over life (colour **and** alpha over life) | H |

Sprites are single images, e.g. `Effects/Smoke/Smoke_01_FX_H3`, `Effects/Glow/Glow_18_FX`, `Effects/Debris/*`,
`Effects/Water/BloodDrop_01`, level FX `Locations/M01/FX/Steam_M01_01`. No flipbook / frame-count property was found
in any template (flipbooks in the data appear to be absent; the engine feature, if any, is unused here), so
`SpriteDesc` supports grid frames for modders but decoded templates always use frame 0 (H for "absent in M01", M overall).

## ZParticleEmitter props (32)

| idx | meaning | conf |
|---|---|---|
| 6 | spawn box half extents (cm) | M |
| 7 | (int16) particles per batch | H |
| 9 | seconds between batches | H |
| 10 | emission window (s) | M |
| 12 | cone spread (deg) | M |
| 13 | initial velocity vec3 in emitter space (cm/s); direction + speed | H |
| 14 | speed variation 0..1 | M |
| 25 | activation message string | H |
| 26..30 | `IsGlobal` (world space sim), `bHide`, `bLighting`, `bAutoStart`, `bDeactivate` (names from the community type registry for the engine class; order matches observed values) | M |
| 31 | template node name | H |

Everything else is kept unmapped. Resolution rate in M01: >90% of emitters find their template in-scene; the
remainder live in other zips and can be merged with `SceneParticles::merge_templates`.

## Simulation model (our implementation)

Semi-implicit Euler: `v += (accel) dt - drag*v dt`, jitter via seeded RNG, colour from the 8-key curve (alpha included),
size = `(start + growth*age) * size_over_life(t)`. Native space is left-handed Y-up, cm. Rendering: additive vs alpha
premultiplied-friendly blend, depth-tested against the scene without depth writes, back-to-front sorted per frame,
batched by (blend, sprite). The engine's exact integration, culling and LOD are unknown (L); the defaults above reproduce the
look in our renders (fire flicker, rising smoke, falling sparks).

## Community knowledge

No public documentation of the Glacier 1 particle node layout was found offline. Names (`ZParticleTemplate`,
`ZParticleEmitter`, `IsGlobal`, `bAutoStart`, ...) come from the type names embedded in the game's PRP data
and the community Glacier type registries; field meanings above were derived empirically (sprite chunk layout,
value ranges per effect name, e.g. `Metal_Sparks` accel `-1000` / stretch `0.015`). Treat all **M/L** rows as open.

## Open questions

- Properties 13/15/16/24/25/26 of templates (likely rotation variation, fade-in/out, texture animation, sort bias).
- Collision/bounce of debris, light emission of `bLighting` emitters, ground-projected decals (blood splats).
- Exact meaning of emitter `duration`/`interval` for one-shot impact effects (we treat `looping = bAutoStart`).
- Whether per-actor emitters (clip control `0x10044e`) are parameterised by weapon data in the `.str` stream.

## Rust API summary

See crate docs: `EmitterDesc` (RON/JSON via `from_ron/from_json`, presets `preset_smoke/sparks/blood`),
`ParticleSystem::{new(seed, capacity), add_emitter, burst, stop, restart, update(dt), collect(&View, &mut RenderList)}`,
`SceneParticles::from_scene(&Scene)` -> `emitters/templates/images`, `render::ParticleRenderer::new(vfs)` ->
`(RenderHooks impl, ParticleHandle)` with `submit(&system, &camera)`, shader override `Shaders/WGSL/particles.wgsl`
(hot reload via `poll_hot_reload`). Examples: `render_particles` (offscreen PNG), `survey`, `templates`, `texsheet`.
