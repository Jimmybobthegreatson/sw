# bm-music - dynamic music

Crate `bm-music` (depends on bm-vfs, bm-sound, bm-models, bm-formats). Confidence legend as in `formats.md` / `sound.md`:
**H** verified against data (cross-checked over all shipped scenes, numeric test), **M** structure verified / meaning
inferred, **?** unknown / guess.

Everything below was derived from the user's own install; no game data is stored in the repo (the generated definitions
only contain file *names* and numbers, never audio).

## 1. How music is stored

### 1.1 Audio data (H)

* All music is **uncompressed streamed PCM** in `pc_eng.str`, addressed by ordinary `.WHD` records of the level banks
  (`Scenes/<L>/<scene>.WHD`, entries named `Music/<name>.wav`, `Music/Licensed/<name>.wav`). Format `fmt 1`, 16 bit,
  mostly stereo 44.1 kHz; licensed tracks are sometimes 32 kHz (`M11_Slasher`, `Tosca_Short`, `TomorrowNeverDies_Mix`);
  some stems/radio mixes are mono (`Tosca_Long_male`, `Music_Cello_M01` 44.1 kHz; `*_Radio` 11.025 kHz).
  Tracks are 6 s (`Death_Sound_01`) to 451 s (`M02_Wonderous`). `bm_music::StrPcmSource` reads them on demand with
  random access (`record[8]` = offset, an optional `LIP ` header of 0x1000 bytes is skipped); verified sample-exact
  against `SoundLibrary::pcm` (test `streamed_source_is_identical_to_the_sound_library_decode`).
* **Flags** of the `.WHD` record (word 1): `0x0080` ordinary stream (635 music/dialogue entries over all banks),
  `0x8280` + `0x0180` always come **in pairs of the same duration**: `0x8280` is the full-range stereo master and the
  `0x0180` entry is its synchronised partner stream - for music a mono stem (`Tosca_Long_Playback` 2ch + `Tosca_Long_male`
  1ch, both 204.5 s) or a lo-fi radio mix (`M06_Salsa_Bomba` 44.1 kHz stereo + `_Radio` 11.025 kHz mono, all 160.6 s);
  for dialogue the same line dry and radio filtered (`M06_MATFA_01AssassinMale` + `...MaleR`). 102 pairs over all banks.
  Interpretation "engine plays both in sync and mixes by distance / room" is **M**.
* 26 banks contain music; 123 distinct music file names across the game. The bulk (≈ 55 files) is the *same global set*
  repeated in every `Mxx_main` bank; the title bank (`Scenes/HitmanBloodMoney`) carries `47 Main Theme`,
  `13 Schubert Ave Maria`, `TomorrowNeverDies_Original` (credits); `Mxx_postmission` banks only `47 Main Theme`.
* **No loop points anywhere** (H negative): the 52-byte `.WHD` record has no unexplained loop field, the `.SND` wave
  record neither, and the stream chunks have no markers. See 1.5 for what the audio itself says about looping.

### 1.2 The per-level 64-slot track table (H)

Each `Mxx_main.SND` (M00-M06, M08-M13; not hideout / M07) holds exactly one object
`audio:_sounddefinitionsbloodmoney/music/music_mXX#def` (kind 5). Its body (`sound.md`: "`#def` undecoded") is:

```
+0x00 u32 5, u32 0xcdcdcdcd, u32 name_ptr, u16 0, u16 0xcdcd
+0x10 u32 0
+0x14 u32 table_ptr        file offset of a pointer table located exactly 0x100 bytes BEFORE the body
+0x18 u16 0x3f, u16 0x3f   (63, 63: highest valid slot index?)
```
The table is **64 × u32**: `slot -> body offset of a #sound` (0 = empty slot). It starts at `body - 0x100` for every level
(12 of 12 levels, all 64 slots, every non-zero entry resolves to a `Sound` object, `bm_music::LevelMusic`). The slot
index is **stable across levels** (same slot = same role) while the sound objects differ per level: either a shared
`music/<name>` or a level specific `music/mNN/mNN_<name>` (same wave file, different volume byte, see 1.4).
This is the "music definition" the native controller indexes (`ZResource_audio_def MusicDefinition`, see 2).

Slot map (M01, volume byte / mode from the `#sound`; `rms` = mean RMS over 24 windows, `tail` = RMS of the last 0.5 s,
measured by `--example track_analyze`; roles M unless noted):

| slot | file | dur s | pct/mode | rms dB | tail dB | role |
|---|---|---|---|---|---|---|
| 1 | Licensed/M02_Wonderous | 450.7 | 39/0 | -15.8 | -180 | ambient bed (licensed) |
| 2 | Exploration | 392.0 | 8/0 | -16.0 | -63 | main exploration bed |
| 3, 53 | Action3_End+Electronics | 77.0 | 39/0 | -13.4 | -62 | combat outro |
| 4 | AmbZone_V2 | 235.4 | 39/0 | -19.0 | -56 | ambience zone (`s_Music_AmbZoneStarted`) |
| 5 | Club_Heaven (no file in M01) | | 17/1 | | | club (M11 uses a mono variant, mode 3) |
| 6 | Licensed/M11_Slasher | 213.1 | 17/1 | -12.7 | -63 | club / hell music |
| 7 | Light_Ambient | 197.3 | 12/0 | -17.3 | -54 | ambient bed |
| 8 | Dark_Ambient | 286.4 | 39/0 | -16.2 | -63 | ambient bed |
| 9 | Apocalypse | 271.3 | 39/0 | -17.7 | -87 | `s_Music_ApocalypseStarted` |
| 10 | Before_The_Storm | 160.2 | 19/0 | -16.6 | -63 | pre-event tension |
| 11, 12 | Hunter_V2 (408.9 s) + Hunter_Intro (62.5 s) | | 39/0, 15/0 | -21.4 | -75 | pursuit: intro then body |
| 13, 14 | Scary_Zone3 (400 s; M01: Scary_Master44_NoIntro 296.5 s) + Scary_Zone3_Intro (103.4 s) | | 39/0 | -19.6 | -60 | scary zone: intro then body |
| 16-18 | Scary_Short_1/2/3++ | 20.2 / 24.8 / 33.0 | 39/0 | -18..-24 | -52 | short scare stingers |
| 19 | Tosca_Short | 71.3 | 9/0 | -17.0 | -71 | opera |
| 20, 40, 59 | 47 Main Theme, 47 Main Theme ingame (no file), 47menuBounce | 185.5 / - / 131.2 | 39/1 | -19.7 | -80 | theme |
| 21 + 22 | Tosca_Long_Playback (0x8280) + Tosca_Long_male (0x180) | 204.5 | 50/0 + 100/2 | -27.9 / -25.6 | -31 / -34 | synchronised stems (playback + singer) |
| 23, 60, 61 | Secret_invasion, SecretInvasion_Suspense++ / +End++ | 296.1 / 46.8 / 134.9 | 39/0 | -19.5 | -64 | invasion suspense (+ end) |
| 24 | Licensed/M10_Salsa_Bomba | 24.3 | 14/1 | -14.9 | -20 | loops (tail as loud as head) |
| 25-27 | M06 Salsa_Bomba / Blues_Double_Trouble / Rock_White_Noise (0x8280) | 160.6 | 39/0 | -14..-18 | -12..-16 | party music, loop |
| 37-39 | the same three, `_Radio` (0x180, 11 kHz mono) | 160.6 | 39/2 | -14..-19 | -20 | positional radio mix |
| 28 | H3AmbientBeats_Master | 118.9 | 15/0 | -16.5 | -61 | beats bed |
| 29, 30 | SuspenseFast / SuspenseSlow | 209.8 / 238.6 | 39/0 | -15.2 | -63 | suspense |
| 31, 32 | SniperLoopSlow / SniperLoop_Fast | 56.6 / 50.3 | 15/0, 39/0 | -26 | -50 | sniper loops |
| 33-35 | Harp+End / Harp_Loop / MixedHarp++ | 134.8 / 46.8 / 55.5 | 39 / 15 / 15 | -19..-31 | -52..-64 | harp: loop + end |
| 36 | Licensed/Music_Ballroom_M09 | 95.9 | 17/0 | -15.7 | -11 | ballroom, loops |
| 41-43 | TomorrowNeverDies mix / vocal / music (only the mix has a file) | 144.1 | 39/0, 39/0(40) | -12.4 | -27 | karaoke (M11) |
| 44 | M13: Funural_Old_Master++ | 164.8 | 15/0 | | | funeral |
| 45-47 | Ethnic_Mix2, Elevator_EQ, ClubHeaven_Master_44fix | 188.6 / 131.2 / 194.7 | 15, 12, 15 | -21..-24 | -44..-64 | diegetic-style beds |
| 48 | Death_Sound_01 | 6.0 | 39/0 | -34 | -51 | death sting |
| 49, 50 | Choir5++ / Choir3++ | 28.0 / 25.3 | 22/0 | -21..-25 | -64..-93 | choir stingers |
| 51 | Music_Cello_M01 (mono) | 105.4 | 63/2 | -10.0 | -64 | M01 cello room (positional, mode 2) |
| 52 | Licensed/13 Schubert Ave Maria | 245.3 | 39/2 | -25.4 | -72 | Ave Maria |
| 54-58 | Action5 / Action1 / Action3 / Action2 / Action1 (dup) | 211.5 / 161.1 / 134.7 / 188.2 | 39 / 39 / 28 / 56 / 39 | -14.5..-18 | -55..-77 | combat |
| 62 | alllevels/credits = TomorrowNeverDies_Original | 255.9 | 22/0 | -11.7 | -180 | credits |

Level variation (`music_list --ron`, `docs`): M04 uses `music/m04/m04_*` objects for 14 slots, M11 for 17 (club / slasher /
karaoke become **mono positional** variants with `mode 3`, volume 100, `range_m 10`), M13 has funeral (44) and its own
Ave Maria (range 11.5 m), M06 has level specific `suspensefast` etc. Slots 53 and 58 are duplicates of 3 and 55.

### 1.3 `#sound` fields that matter for music (M / ?)

From `SndDb::sound_params` (`sound.md`): `volume_pct` (+0x27), `mode` (+0x28), `aux` (+0x29), `range_m` (+0x18).

* **Volume byte** is a *per-level mix level of that music*: the level specific `music/m04/m04_action1` reuses the very
  same wave as M01's `music/action1` with 31 instead of 39; `hunter_v2` is 19 in M04/M00 but 39 in M01; Exploration is 8 (M)
  while the measured mean loudness of the files is within ±5 dB. The generated defaults store `byte / 39` (39 is the
  modal value) as `TrackDef::trim`; it is *not* applied when a loose mod file replaces the audio.
* `mode`: 0 normal, 1 on club / slasher / main theme / M10 salsa (?), **2 = diegetic / positional** (cello, male vocal,
  radio mixes, Ave Maria) and 3 in M11's positional club mixes; positional sounds have a larger `range_m` (3 / 8 / 10 / 11.5
  versus 2.5 for score). `aux` = 25 on the radio mixes, 40/70/50 on karaoke/vocal/club (?). (M for mode 2/3, ? otherwise.)

### 1.4 What the audio itself says about looping and transitions (H, numeric)

`track_analyze` (head / tail RMS and the wrap-seam step relative to the median sample step) over the slot table of M01:

* **Nearly every track fades to silence at its end** (tail -50 .. -180 dB against -13 .. -25 dB average). They are *not*
  authored as seamless loops: the native controller plays one file and picks the next (a playlist per mood), or fades
  between them. This is why the generated defaults model moods as **playlists of one-shot cues**.
* The exceptions are exactly the diegetic dance/party material: `M06_Salsa_Bomba`, `M06_Rock_White_Noise`,
  `M10_Salsa_Bomba`, `Music_Ballroom_M09` (tail within 6 dB of the head and a wrap-seam step ≈ one normal sample step, ratio
  0.0 - 11) - those are authored to loop (the generated defaults loop them, hard wrap for ratio ≲ 5, 0.4 s crossfade
  otherwise).
* Files named `...Loop` (`Harp_Loop`, `SniperLoop*`) and `Hunter_V2` also fade out; they are looped with a 2 s equal-power
  crossfade by the defaults (a fallback choice, not original behaviour).
* Intros: `Hunter_Intro` (62.5 s) precedes `Hunter_V2`, `Scary_Zone3_Intro` (103.4 s) precedes `Scary_Zone3`; the
  `...+End++` / `..._End` files are outros. Sequencing is native (2).

## 2. Where the cue logic lives (what is *not* data)

* `HitmanBloodMoney.exe` contains RTTI/strings `ZMusicController`, `ZMusicControllerBase`, `ZMusicLocations`,
  `hitman3\levelcontrol\musiccontroller.cpp`, `ZGEOM_MusicLocationDef`, `Audio_Set3DMusic`, a level-control class field
  `ZResource_audio_def MusicDefinition` and the settings `DisableMusic`, `Music_User`, `Audio_Volume_Music`,
  `TitleMusic_PausedTime`, `ExternalSources`.
* The per-level behaviour is compiled C++ (`Scriptcs/_gamerelease/Mxx.dll` and the exe; the DLLs contain no readable music
  names). Exported/visible event names (strings only, no disassembly - **M** for "these are the triggers", **?** for what
  each does):
  `g_MusicControllerM01__Event__{BreakUtilBox, ChangedClothes, ChangedClothes_Guard, ChangedClothes_Guard_VIP, ChangedClothes_Worker}`,
  `g_MusicControllerM08__{Event__BrokeUtilsBox, Event__Cake, Event__Pictures, Event__VIPpass, Killed_GangMembers}`,
  `g_MusicControllerM09__Event__{ChangedClothes__Guests, ChangedClothes__Priest, PickupInvitation, PlacedBomb, WeddingBell}`, and
  ~45 `s_Music_*` state flags (`AlarmOn`, `AmbZoneStarted`, `ApocalypseStarted`, `FadeBackAmbZone`, `FadeBackVegas`,
  `FakeCall`, `InElevator`, `FirstTimeOn7thFloor`, `HasGotKeycard`, `LaserBeamFront`, `OnTopOfElevator`, `SaveZone`,
  `Zone5_Counter`, `Zone6_Counter`, kill flags for each target (`SheikKilled`, `OperaSingerKilled`, ...), `M04_*`,
  `ZHM3LevelControlM11::s_Music_PlayKaraoke` ...). They map one-to-one onto the slot names (`AmbZoneStarted` = slot 4,
  `ApocalypseStarted` = slot 9, `PlayKaraoke` = slots 41-43).
  So the original design is: **zone + mission events -> native state -> slot number -> `#sound`**.
* Therefore the *verifiable* data model is the slot table (1.2), the zones (3) and the diegetic emitters (4). The mapping
  event -> slot is not in data; `bm-music` ships a documented **fallback** (section 7) and a mod-authorable data format
  for exactly that mapping.

## 3. Zones: `MusicLocationDef` (H structure, M meaning)

`MusicLocationDef` controllers (`sound.md`: `[Container rooms, I32 zone, Container []]`, 102 nodes) define **music zones**:
zone id = the number the native controller switches on (`s_Music_Zone5_Counter`...), rooms = the room names that belong
to it. Counts per level: M00 4, M01 9, M02 9, M03 5, M04 7, M05 11, M06 6, M08 8, M09 7, M10 16 (ids 0-18 with gaps), M11 15, M12 4, M13 1.
Node names carry the intent, e.g. M01: `Zone_0__No_Music`, `Zone_1__Cave`, `Zone_2__Inside_Cellar__Room_With_Dons_Son`,
`Zone_3__Cellar`, `Zone_4__DrugLab`, `Zone_5__Drop_Down_From_Roof`, `Zone_6__CelloRoom`, `Zone_7__MainHouse_1stFloor`,
`Zone_8__MainHouse_2ndFloor`; M06 `01_PartyBlues`, `02_PartyRock`, `03_PartySalsa`, `04_Hotel_Upstairs`, `05_Book_store`;
M09 `Zone_01__Dancing`, `Zone_06__BallRoom`; M10 `Zone_00__Silent_Rooms`, `Zone_09__Bar`, `Zone_04__Elevators`;
M08 `Zone_0__NoMusic`, `Zone_1__StopMusic`. Silent zones (names with "no music / stop music / silent") are reliable.
The listener-in-room test uses the decoded `.RMI` map (`bm_game::rooms`, `docs/formats.md`): bm-game reports the current room and calls `MusicSystem::set_zone` (`AudioDirector::follow_room`);
`LevelMusic::zones[i].rooms` gives the room names for that lookup (`ZoneInfo`, room = last path segment).

## 4. Diegetic ("3D") music (H structure)

Music that comes out of a radio / singer / TV is a normal positional emitter: `ZSNDOBJ` + controller `Audio_Set3DMusic`
(4 nodes), usually with `SoundLimiterFade` (audible only while the listener is in the listed rooms; music nodes use
fade-in 1.0 s / fade-out 4.0 s versus 3 s / 3 s for sfx): M01 `Mulher_Dulce_YardMusic`, `CelloMusic`; M02 `Grotto_Music`;
M08 piano; M09 `Music_Frontyard`, TV music; M10 TVs; M11 `3D_Music!{HeavenMusic, HellMusic, Karaoke!Music}` (mono mixes
`m11_club_heaven`, `m11_slasher`, `m11_tomorrowneverdies_mix`). These belong to `bm_sound::SoundScene`; `bm-music` only
provides the non-positional score and the pair/stem material (`tosca` cue, party tracks with their `_Radio` partners).

## 5. Data model (`MusicDef`, RON or JSON)

All structs `#[serde(default)]`: write only what you need. RON accepts implicit `Some` (`ducking: (voice_depth: 0.3)`).
`MusicDef::parse(text, Format::Ron|Json)`, `to_ron()`, `to_json()`, `merge(other)`, `validate()`. Reference:

* **tracks** `{key: TrackDef}`: `source` = `Bank("Music/Exploration.wav")` | `Slot(n)` | `File("Music/MyMod/x.ogg")`;
  `gain` (authored), `trim` (original level, ignored for replaced audio), `looping` (default true; whole file unless a loop
  region), `loop_start_secs`/`loop_end_secs` or sample-exact `loop_start_frames`/`loop_end_frames` (audio before
  the start plays once), `loop_crossfade_secs` (equal-power blend of the tail into the head; period = `end - start -
  xfade`), `bpm`, `beats_per_bar`, `beat_offset_secs`.
* **cues** `{name: CueDef}`: `stems: [StemDef]` start together and stay **sample aligned** (a cue stalls as a whole when any stem
  starves, never drifts), `gain`, `start_secs`. `StemDef`: `name`, `track`, optional `intro` (track played once, then
  `track` loops, gapless), `gain`, `layer: LayerDef { fade_in: (a, b), fade_out: Option<(c, d)>, curve }` mapping the
  **intensity** (0..1) to the stem gain (`0` below `a`, `1` above `b`, ...).
* **states** `{name: StateDef}`: `cue` (None = silence), `playlist` (more cues after it, wrap-around; with `shuffle`,
  `playlist_fade_secs`, `playlist_overlap_secs` = start the next one early as a crossfade; applies to non-looping
  cues), `on_finish` (state entered when a one-shot cue ends), `intensity` (set on entry), `gain`, `stem_gains` (per-stem
  overrides, e.g. mute the drums), `min_hold_secs`, `zone_locked`, `follow_zone`.
* **transitions** `[TransitionDef { from, to, rule }]` (empty list = any; first match wins, mods prepend) and
  `default_transition`. `TransitionRule`: `fade_out`/`fade_in: FadeDef { secs, curve }`, `sync: SyncMode`
  (`Immediate | Beat | Bar | LoopEnd`), `max_wait_secs` (fire early if the sync point is further away), `overlap` (false = the new cue
  starts after the old has faded), `delay_secs`, `stinger`.
* **curves** `Curve`: `Linear | EqualPower | SCurve | Quadratic | Sqrt | Exp(k) | Points([(x, y)])`. Rising gain curve;
  fades out use the mirror (so `EqualPower` crossfades keep constant power).
* **zones** `{zone id: state}`, **events** `{name: EventAction { state, stinger, intensity, intensity_delta }}`,
  **stingers** `{name: StingerDef { track, gain, duck_depth, duck_attack_secs, duck_release_secs, fade_in_secs }}`,
  `ducking` (engine-level Voice -> Music rule, applied by `attach`), `intensity: IntensityDef { smooth_secs, initial }`,
  `master_gain`, `initial_state`.

Examples: `bm-music/examples/data/mod_minimal.ron` (replace combat with one file, beat-synced) and `mod_layered.ron`
(three stems driven by intensity; all states share the cue so state changes only re-balance it). Both are parsed, merged and
validated by the test-suite.

### Runtime semantics (`Conductor`)

* `set_state(name)`: picks the transition rule `from -> to`; computes the fire time (`sync` against the *playing* cue's timeline:
  next beat/bar from `bpm`, next loop boundary or end of intro; capped by `max_wait_secs`; not before `min_hold_secs` of the current
  state unless `force_state`); **prepares** the target cue (opens/decodes in the read-ahead worker, prebuffers) while the old
  one keeps playing; at the fire time (and once prepared, at most `max_prepare_secs` later) the old cue fades out and the new one
  fades in with the rule's curves, sample accurately (the mix loop is split at event boundaries, chunks ≤ 256 frames).
  Asking for the current state cancels a pending move; a later request replaces an earlier pending one.
* Same cue in the target state: nothing restarts; stem gains are re-balanced over `fade_in`.
* `follow_zone`/`zone_locked`: `set_zone(z)` is remembered; locked states (combat, alert...) ignore it, and entering a
  `follow_zone` state (explore) hands over to the remembered zone's state.
* Intensity: `set_intensity` moves a smoothed value (≤ `1/smooth_secs` per second); stem gains follow it with per-chunk linear
  ramps (no zipper noise).
* Ducking: stingers with `duck_depth < 1` duck the music (not themselves) with attack/release; `duck(depth, attack, release)` /
  `unduck()` for manual ducking; `attach` also installs the Voice -> Music bus ducking from `MusicDef::ducking`.
* `pause(fade)` fades out and **freezes the clock** (pending transitions, playlist timers); `resume(fade)` continues from
  the exact sample.
* Events: `StateChanged, TransitionScheduled, CueStarted, CueFinished, StingerStarted, TrackError, Ignored`
  (`poll_events`). Missing tracks degrade to silent stems + `TrackError`, never a panic.

### Streaming and real-time safety

Every stem owns a ring of decoded stereo frames at the engine rate, filled by a single prefetch thread
(`read_ahead_secs`, default 1.5 s). The worker does all I/O: `pc_eng.str` reads, loose-file decoding (symphonia),
loop/crossfade generation and Catmull-Rom resampling (bit-exact pass-through when the rate matches).
The audio callback (`MusicMixer::read`, a `PcmSource` on `Bus::Music`) only pops frames from the rings and mixes; if a ring runs
dry the whole cue stalls for that chunk (counted in `stats().underruns`) so layers never drift. Manual mode
(`MusicOptions::offline`) pumps the rings inline: fully deterministic offline rendering. Caveat: a few small `Vec`s per
chunk are still allocated in the mix loop, and the conductor mutex is shared with the game thread (held for microseconds).

## 6. Modding

Loose files through the VFS (`Vfs::add_mod_dir`, later dirs win). `<level>` is the scene directory (`M01`, `hideout`),
`_global` applies to every level; level specific beats global, later mod beats earlier mod.

| path | effect |
|---|---|
| `Music/<level>/<cue>.<stem>.{ogg,flac,wav}` | replaces one named stem of cue `<cue>` |
| `Music/<level>/<cue>.{ogg,flac,wav}` | replaces the **primary (first) stem** of cue `<cue>` - the common case |
| `Music/<level>/<track>.{ogg,flac,wav}` | replaces a track by its key in the definition (every cue that uses it); original tracks are keyed by their lower-case file name without extension and spaces as `_` (`exploration`, `m06_salsa_bomba`, `47_main_theme`) |
| `Music/_global/...` | the same three forms for all levels |
| `Music/<level>/<cue>.<stem>.intro.ext` | replaces the intro of a stem |
| `Music/<level>/*.ron`, `*.json` and `Music/_global/*.ron|json` | custom definitions, merged over the generated defaults in the order global -> level, RON before JSON, alphabetical. A file that fails to parse or would make the merged definition invalid is **skipped with a warning** (`LevelMusicSet::warnings`) |

Original cue names in the generated defaults equal the track key (`exploration`, `action1`, `hunter`, `scary`, `tosca`), so
`Music/M01/exploration.ogg` replaces the exploration bed of M01. Replacement audio may have any sample rate / channel count
(resampled, mono is duplicated). Replaced audio is used at unity gain (`gain`), not the original `trim`. `bm-sound`'s
`Sounds/<bank stem>/<name>` overrides are also honoured for bank sounds. Loose files are decoded fully into memory by the worker
(a 4 minute stereo track ≈ 40 MB of i16); only the original PCM music is streamed from disk.
Get a template of everything the game defines: `music_list --ron [scene]`.

## 7. Defaults: the fallback "original" behaviour (`default_level_def`)

Because the real cue logic is native, `default_level_def(&LevelMusic)` builds an explicit, editable fallback (all **?**,
documented here so mods can replace any piece). Per-moods playlists of whatever slots exist in the level:

| state | cues (slots) | notes |
|---|---|---|
| `silent` | - | zone kind Silent; also the initial state of banks without music |
| `explore` | 2, 7, 4, 8 (+28, 10) first present, up to 4 as a playlist | overlap 6 s, `follow_zone`, initial |
| `suspense` | 30 SuspenseSlow, 29 SuspenseFast | locked |
| `scary` | intro 14 -> body 13 (stem `intro`) | locked |
| `alert` | intro 12 -> 11 Hunter (else 23) | locked, hold 4 s |
| `combat` | 55, 57, 56, 54, shuffled playlist | locked, hold 6 s, overlap 3 s |
| `sniper` | 31, 32 | on event `sniper` |
| `complete` | 20 (47 Main Theme) | |
| `failed` | silence + `death` stinger (48, ducks the music) | |
| `party_salsa/blues/rock`, `ballroom`, `cello`, `karaoke`, `opera` | 25, 26, 27, 36, 51, 41, stems 21+22 | zone states chosen by zone-name keywords (`salsa`, `blues`, `partyrock`, `ballroom|dancing`, `cello`); every other music zone -> `explore`, silent zones -> `silent` |

Mission hooks (`MissionEvent` -> `events`): `mission_start`, `calm` -> explore; `suspicious` -> suspense; `alert` -> alert;
`combat` -> combat; `target_killed` -> stinger `choir_short`; `mission_complete` -> complete; `mission_failed`, `player_dead` ->
failed. Paused / Resumed map to `pause()` / `resume()`. Transition defaults: into combat 0.8 s, into alert 1.5 s, out of combat /
alert / suspense to explore 5 s fade out + 4 s fade in, into silent 2.5 s S-curve, otherwise 3 s equal-power.
Banks without a music table (front end, premission, postmission) use `default_bank_def`: one state per `Music/*` file, the
main theme loops.

## 8. API summary

```rust
// level data
let set = bm_music::load_level_def(vfs.clone(), lib.clone(), "Scenes/M01/M01_main")?; // LevelMusicSet { level, def, resolver, merged_files, warnings }
let mut music = set.system(MusicOptions::for_engine(&engine))?;      // MusicSystem
music.attach(&mut engine)?;                                          // play_stream on Bus::Music + voice ducking rule
music.set_zone(6); music.mission_event(MissionEvent::Combat); music.set_state("alert");
music.set_intensity(0.7); music.trigger_stinger("death"); music.pause(0.4); music.resume(0.4);
for ev in music.poll_events() { /* StateChanged, CueStarted, ... */ }
// offline
let pcm = music.render(48_000 * 10);   // interleaved stereo f32
```
`MusicSystem::new(def, resolver, opts)` works with any `TrackResolver` (`GameResolver`, `MemoryResolver` for procedural/test audio).
`bm_music::analysis::{check_boundaries, steps, quantile, basic_stats}` implement the click detector used by the tests.

## 9. Tools and tests

* `cargo run --release -p bm-music --example music_list -- [scene] [--ron|--json] [--mod dir]` - slot table with roles, zones, states.
* `... --example music_render -- [scene] [out.wav] [seconds]` - scripted session through `MixerEngine::headless` (explore -> combat ->
  calm -> suspense -> alert -> calm, pause/resume), writes a WAV, prints events and a per-transition click check; exit code 1 on
  failure. Reference run (M01, 70 s): 215x real time, peak 0.92, 0 underruns, worst boundary ratio 1.66 (limit 3).
  `--loops` renders the 3 s around the wrap of every looping track and checks the seam (all ok on M01 and M06).
* `... --example music_play --features device` - interactive player on the default output device.
* Research: `music_survey [banks|nodes|snd|defs]`, `snd_dump`, `hexat`, `track_analyze`.
* Tests: `cargo test -p bm-music` - 29 unit tests (curves, loops, crossfades, resampler, defs, zones), 25 engine tests on synthetic
  signals (state machine, sync, min-hold, crossfade click-freedom and equal power, loop crossfade, intro, layers, pause
  freeze/resume continuity, ducking, stingers, playlists, zones, threaded read-ahead without underruns, the headless mixer
  path), 5 modding tests (override precedence, stems, merge, broken files) and 7 real-file tests (skip without the game): all
  12 main levels build a valid definition and every track opens; the streamed source equals the library decode; scripted session
  without clicks / underruns; loop wraps; threaded streaming; loose replacement on a real level; every scene of the install.

## 10. Open questions

* The native event -> slot mapping per level (needs disassembly of `ZMusicController` / `Mxx.dll`); whether the engine crossfades
  or hard-switches, and its fade times.
* Meaning of `#sound` `mode` 1 and `aux` values, exact semantics of the volume byte (per-level level of the music is plausible).
* Positional music range falloff in the original (`range_m`, `SoundLimiterFade` rooms) - belongs to bm-sound/bm-game.
* Which of `Harp_*`, `*_End*` tracks chain in the original (outro after loop); the `TomorrowNeverDies` vocal/music slots (42/43)
  have sound objects but no bank entries in the shipped banks.
