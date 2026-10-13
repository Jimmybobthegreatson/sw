# bm-sound - audio

Crate `bm-sound` (depends on bm-vfs, bm-formats, bm-models). Confidence legend as in `formats.md`:
**H** verified against data (round trip / cross-check over all shipped scenes), **M** structure verified,
meaning inferred, **?** unknown / guess.

Modules: `library` (banks, cache, mod overrides, lip), `snd_db` (`.SND` object graph), `scene` (sound nodes),
`mixer` / `reverb` / `engine` / `device` (playback), `player` (glue), `decode` (symphonia), `pcm`, `lip`.

## Data model

* A level has `Scenes/<L>/<scene>.WHD` + `.WAV` (bank), `.SND` (sound object graph) and shares the global
  `pc_eng.str` stream. `SoundLibrary::open_level(vfs, "Scenes/M01/M01_main")` loads bank + `.SND`;
  `load_bank` adds more banks (`SoundId { bank, index }`).
* **Frame counts** (**H**): `.WHD` `sample_count` (record word 9) counts samples over *all channels*; stereo music
  has `frames = sample_count / 2`. (Mono dialogue/effects: equal.)
* **Streamed entries** (**H**): `SoundEntry::is_streamed()` in bm-vfs misses PCM music (`flags 0x8280`, record[0]
  = 0, offsets far beyond the `.WAV`); `bm-sound` also treats flag bit `0x8000` as streamed. Streamed 16-bit PCM
  stereo music (44.1 kHz, `fmt 1`) is read straight from `pc_eng.str` at `record[8]`, `record[6]` bytes; dialogue
  is IMA ADPCM, mono, odd rates (32226, 34628, 37182, 44100 Hz...).
* **Flag bit 0x80** ("PHO") is set on *all* streamed entries, music included - it does not mean "has lip data".
  `SoundLibrary::lip_sync(id)` checks for the `LIP ` tag.
* Decoded PCM is cached (`Arc<Pcm>`, 16-bit interleaved, LRU, 256 MiB default budget).
* Numeric sanity (tests `bm-sound/tests/real.rs`): decoded lengths equal the directory for resident and
  streamed lines; 200 resident sounds: mean clipped-sample fraction < 1 %, dialogue zero-crossing rate < 0.3
  (white noise would be 0.5, i.e. a broken decoder), non-silent.

## Lip sync chunks (`"LIP "`, 0x1000 bytes before each dialogue stream) - M

```
0x00 "LIP "
0x04 u32 N                 (= payload bytes - channel_count; ?)
0x08 u16 channel_count     8..15
0x0a ?                     6 bytes (f32 + u16 pair, mostly 0)
0x10 channels: u8 id, u8 id (repeat), u16 key_count, key_count x { u16 time, u8 weight }
```
All 939 lines of the M01 bank parse (channel count + key counts consume the block exactly). Channel ids are
`0`, `14..24`, `40..43` (viseme slots; the mapping to `.ANM` facial poses is **?**). `time` unit = **1/24 s**:
last key time / clip duration over 939 lines is 24.0 .. 24.8 (p10..p97), i.e. keys end where the audio ends.
`weight` 0..255 mouth weight. API: `LipSync::parse`, `weight_at(id, t)`, `frame_at(t)`, `SoundLibrary::lip_sync`.

## `.SND` object graph (`snd_db`)

Flat memory image; every pointer is an absolute file offset. Objects have names `audio:<path>#<kind>`; kinds over
all 68 scenes: `sound` 78 945, `wave` 57 758, `seq` 3 984, `set` 3 326, `surfaceproperty` 1 224, `def` 917,
`roomeffect` 828, `group` 655, `objectproperty` 611 (the old `docs/formats.md` list was missing the last four).

* **Body header** (**H**, found for 100 % of names except `wave`/`roomeffect`): 16-byte aligned,
  `u32 kind, 0xcdcdcdcd, u32 name_ptr, u16 n, u16 0xcdcd` where `name_ptr` = offset of the `audio:` string and `n` is a
  child count (not the number of waves). `kind`: 1 sound, 2 set, 3 seq, 0x12 objectproperty, 0x13 surfaceproperty.
  **Scene nodes reference objects by the offset of this header** (see below).
* **Wave record** (**H**): follows a `#wave` name + the original file name (`Action/Wipe_Table/Wipe_Table_01.wav`):
  `u32 4, u32 x (name length for resident waves, constant 0x01d5bc98 for streamed ones), u32 file_name_ptr, 0, 0,
  u32 whd_record_offset, f32 duration_seconds, 0, u32 1, cdcdcdcd, u32 sound_name_ptr, ...`.
  - `duration_seconds` (+0x18) equals `frames / rate` of the bank entry (all 1532 M01 waves within 10 ms).
  - `sound_name_ptr` (+0x28) joins a wave to its `#sound`. 56 659 of 56 659 wave records over all scenes join a bank
    entry by file name (`SoundBank::find`).
  - +0x38 `f32` (1.0 usually; 2.0, 0.5, 1.5 seen) per-wave factor **?**.
* **Sound body** (parameters, **M**/**?**): `+0x18 f32` (10, 14, 35 ...: probably the reference distance in metres
  **?**), `+0x1c i16 / +0x1e u16 360 / +0x20 u16 360 / +0x22 i16` = (gain, cone inner, cone outer, gain) - the two
  cones are 360 in every sound (omni), one of the two `i16` is -60/-100 and the other 0 (**?**), `+0x27 u8`
  **volume percent** (100, 50, 17 ... **M**, 79 k sounds), `+0x28 u8` (1 for ambient loops, 2 for one-shots, **?**),
  `+0x29 u8` (0x14/0x1e/0x23/0x19 on dialogue, 0 on effects, **?**). Exposed as `SoundParams`.
* **`#roomeffect`** (**H** layout, 564 of 828 objects carry a block, the others are bare names): a block of 24 words
  starting 0x10 after the 16-aligned end of the name, in `EAXREVERBPROPERTIES` order minus the three pan vectors:
  `u32 ?, u32 2, u32 ?, f32 size (7.5), f32 diffusion (1.0), i32 room (-1000), i32 roomHF (-100), i32 roomLF, f32 decay (1.49),
  f32 decayHF (0.83), f32 decayLF, i32 reflections (-2602), f32 reflectionsDelay (0.007), i32 reverb (200), f32
  reverbDelay (0.011), f32 echoTime (0.25), f32 echoDepth, f32 modTime (0.25), f32 modDepth, f32 airAbsorptionHF (-5), f32
  hfRef (5000), f32 lfRef (250), f32 rolloff, u32 flags (0x3f)`. The defaults equal the standard EAX "Generic"
  preset, which is how the layout was identified. Rooms without a block fall back to "no reverb". API:
  `SndDb::room_effects`, `ReverbParams::from_eax` (decay -> RT60 combs, decayHF -> damping, room+reverb -> wet).
  Room effects belong to `ZROOM` nodes by the last path segment (`...m01_hacienda_room_101` <-> node
  `M01_Hacienda_Room_101`), `SoundScene::room_effect`.
* **`#surfaceproperty`** (footstep/impact materials, e.g. `materials/surface/glass`, `default`): body header kind 0x13 +
  words (`[0, 1, -10000, 0, 1.0f, ..., 8, -200, 50, ...]`), first 16 words kept as `SurfaceProperty::words` **?**.
  `#group` `character/footsteps/footsteps` is the footstep tree (names `character/footsteps/<material>/footstep_<material>_00N`
  sounds; wave count per material visible via `SndDb::waves_of`).
* **`#def`** (`_sounddefinitionsbloodmoney/characters/m01/don`): per-character sound definition tables, 44 objects; the `music/music_mXX` def is a 64-slot track table (decoded in `docs/music.md`); other bodies
  undecoded **?**.
* `seq`/`set`/`group` bodies: pointer tables to sounds with float parameters (`0xe, 0.32, 0.08` = random volume/pitch
  variation?) **?**. Sets (e.g. `impacts_bullet/glass/glass`) are what `ZSNDOBJ`/`Audio_PlaySound` reference for
  surface impacts; selecting the member sound is not decoded - use `SoundPlayer::play_object` on a `#sound`.

## Scene nodes (`scene::SoundScene`)

Survey over all 68 scenes (types from `typesurvey`, counts are node counts):

| type | name | n | notes |
|---|---|---|---|
| `0x200012` | **ZSNDOBJ** | 1527 | emitter. `props[6]` = `.SND` body offset (**H**: 1512 resolve, 15 are 0, none dangling). Targets: `#sound` (e.g. `levelsetup/m01/water/waterfall`), `#set`, `#group`. Names like `M01/M01_Audio.zip!Music!...`, `Sounds_3D!Pier_Ocean`, `ShatterSound` (lamp shatter) |
| `0x2000e1` ZBoxPrimitive | e.g. `BombExplodingSound` | 3119 | carry `Audio_PlaySound` |
| `0x100021` ZROOM | rooms | 1507 | key for room reverb / `SoundLimiterFade` / `MusicLocationDef` lists |

Audio controllers on nodes (counts over all scenes; all resolved I32 properties point at real `.SND` objects):

| controller | n | props (confidence) |
|---|---|---|
| `Audio_PlaySound` | 193 | `[I32 snd offset, Str event...]` events seen: `Activate`, `Deactivate`, `ProjectileHit` (H structure, M semantics) |
| `Audio_AttachToGeom` | 255 | `[I32 snd offset]` (offset 0 = none) - sound owned by a geometry node (coffee machine, lamp) |
| `Audio_Definitions` | 27 | `[I32 #def offset, I32 0]` |
| `Audio_Set3DMusic` | 4 | `[]`, on ZSNDOBJ: positional music source |
| `MetalDetectorAudioDef` | 11 | `[I32 loop sound, I32 found-weapon sound]` |
| `SoundLimiterFade` | 187 | `[Container rooms, Container [], F32 3.0, F32 3.0, Bool, Bool]`: sound is audible only while the listener is in the listed rooms, fading over the two floats (M; music uses 1.0 / 4.0) |
| `SoundLimiter` | 2 | same family, no fade |
| `MusicLocationDef` | 102 | `[Container rooms, I32 zone, Container []]`: music zone N (`Zone_3__Cellar`) covering the rooms (M; for bm-music) |
| `SoundSpline` | 3 | `[Str spline node, F32 x4]` (?) |
| `SoundSplineFollow` | 13 | `[Str spline, Token RANDOM|RANDOM_AND_MOVE, F32 rate, I32 count, Bool]`: point sources spread along a spline (M) |
| `SoundVolumetricSource` | 10 | 11 values (`30, 15, 455, true, 0, 0, 1.5, 0.5, 0.85, 30, true`) ? |
| `SoundVolumetricRandom` | 6 | `[F32 3000, 100, 1500]` ? |
| `ScriptC` / `CheckInside*` / `FireAlarm` | | gameplay scripts that trigger sounds, handled by bm-script / bm-game |

`SoundScene::from_scene(&bm_models::Scene, &SndDb)` returns every ZSNDOBJ and every node with one of the controllers
above as `SoundNode { position (cm, world), sound: Option<SoundRef>, behaviours: Vec<SoundBehaviour> }`.
Not available: sound volumes/ranges *per emitter node* (only per `#sound`). The listener-in-room test now exists: `bm_game::rooms::RoomIndex`
(decoded `.RMI`, `docs/formats.md`) reports the room of the listener and `bm_game::AudioDirector` applies the room's EAX preset
(`ReverbParams::from_eax`) and music zone when the room changes; `SoundLimiterFade` audibility by room list is still not applied.

## Mixer / engine

`AudioEngine` (object safe trait): `play(Arc<Pcm>, VoiceParams) -> Option<VoiceHandle>`, `play_stream(Box<dyn
PcmSource>, ..)`, `stop(h, fade)`, `update_voice`, `set_listener`, `set_bus_volume/muted`, `set_ducking`, `set_reverb`,
`set_max_voices`, `stats`. Implementations: `MixerEngine` (shared `Arc<Mutex<Mixer>>`, headless by default),
`NullEngine` (records calls). With `--features device`, `device::DeviceOutput::start(engine.mixer())` /
`start_default()` streams the mixer to the default cpal device (stays on its creating thread).

* Voices: buffer (linear-resampled, pitch, loop) or stream; priority, voice stealing (lowest priority, then quietest,
  then oldest; a new voice only steals voices of priority <= its own, otherwise `None`), fade in/out, per-voice
  volume/pan/occlusion/reverb send.
* 3D (`mixer::spatialize`, public): world units are caller defined (`Spatial::from_meters` converts native cm);
  listener basis is the Glacier left-handed Y-up one (right = up x forward). Gain = inverse distance clamped
  between `min_distance` and a smooth fade to 0 at `max_distance`; constant-power pan from `sin(azimuth)` (narrowed
  for near sources); behind-listener sources lose up to 30 % level and brightness; one-pole low-pass models air
  absorption + occlusion. No HRTF, no Doppler.
* Buses: `Sfx, Voice, Ambient, Music, Ui` + master; volume/mute, ducking rule (`Voice` ducks `Music`). **Music hook for
  bm-music**: `engine.play_stream(Box::new(source), VoiceParams::on_bus(Bus::Music))`, `PcmSource` = interleaved f32 at any
  rate/channels (resampled to the output rate).
* Reverb: shared stereo comb/all-pass reverb fed by per-voice `reverb_send`; `engine.set_reverb(ReverbParams::
  from_eax(room_effect))` when the listener changes room.
* Soft limiter on the master (never reaches 1.0, verified by a stress test with 20 full-scale voices).
* Headless: `MixerEngine::render(frames)`, `render_offline(seconds, block, |t, engine| ...)` (scripted, deterministic),
  `MixerEngine::write_wav`.

## Modding (loose files)

Through the VFS (mod dirs added with `Vfs::add_mod_dir`, later dirs win). For a bank with stem `M01_main` and the sound
`Action/Wipe_Table/Wipe_Table_01.wav` at index 12, the first existing file of
1. `Sounds/M01_main/Action/Wipe_Table/Wipe_Table_01.{ogg,flac,wav}`
2. `Sounds/M01_main/id_12.{ogg,flac,wav}`
3. `Sounds/_global/Action/Wipe_Table/Wipe_Table_01.{ogg,flac,wav}` (all banks)

replaces the bank data (any sample rate/channel count; decoded with symphonia: WAV incl. ADPCM/float, Ogg Vorbis,
FLAC). `SoundLibrary::pcm_by_name` also finds *new* names that only exist as loose files (`Sounds/_global/...`,
`Sounds/<stem>/...`); `list_mod_sounds()` enumerates them. Tests: `tests/modding.rs` (no game needed) and
`tests/real.rs::loose_file_overrides_bank_sound_by_name_and_by_id`. Not overridable yet: lip-sync data (the
original `LIP ` chunk keeps being returned by `lip_sync` even when the audio is replaced; a loose `.lip` loader is a
TODO), `.SND` parameters (volumes) and room effects.

## Tools / examples

* `cargo run --release -p bm-sound --example play_sound -- list|info|dump|sample|play ...`
* `cargo run --release -p bm-sound --example render_mix -- [scene] [out.wav] [seconds]`: 12 emitters of the M01 scene
  (densest cluster) + scripted shots/explosion/dialogue around a walking, turning listener, reverb from the first EAX
  preset; prints peak/rms/clip/active/DC/LR numbers and exits non-zero on failure.
* Research: `sound_survey` (all scenes), `ctl_survey <scene> <controller>`, `snd_stats`.

## Open questions

* Meaning of `#sound` bytes `+0x26/+0x28/+0x29`, `+0x18`, `i16` pair; the 3D block `[0x19, 10.0, 40.0, flags]` that
  follows some sounds (min/max distance?).
* `seq/set/group` selection logic (random/sequence/limit), `#def` and `#objectproperty` bodies, `#surfaceproperty`
  fields; mapping of surface ids to footstep sets.
* Meaning of LIP channel ids; `N` header word and the 6 bytes at `0x0a`.
* `.RMC/.RMI` room maps (needed to know which room the listener is in).
* Community info: no web search was needed; ReGlacier/HBM_GMSTool (already used for type ids) does not cover sound.
