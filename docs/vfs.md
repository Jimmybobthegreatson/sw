# bm-vfs - virtual file system

Crate: `bm-vfs`. Reads the user's own Blood Money installation (never copies data into the repo).
Game dir: `$BM_GAME_DIR`, default `C:\Users\Shadow\Downloads\Hitman Blood Money`.

## Install layout (observed)

```
HitmanBloodMoney.exe, HitmanBloodMoney.ini, *.dll ...
pc_eng.str                         1.8 GB voice / music stream (see below)
Movies/*.bik                       Bink videos (not handled here)
Scriptcs/                          script modules (_gamerelease, M00_M02P)
Scenes/HitmanBloodMoney.{ZIP,WAV,WHD}      front-end ("boot menu") scene
Scenes/saveandcontinue.{ZIP,WAV,WHD}
Scenes/hideout/{hideout_main,Loader_Sequence}.{ZIP,WAV,WHD}
Scenes/M00 .. M13/<scene>.{ZIP,WAV,WHD}   scenes: Mxx_main, Mxx_premission, Mxx_postmission,
                                          Mxx_news, Loader_Sequence (+ M00_intro/albino, M13_intro)
```

`HitmanBloodMoney.ini` (plain text, `;` comments): `DefaultScene=M01/M01_main.gms`, `streamfile pc_eng.str`,
`ReadMainIni 0`, `[CONFIGURATION]` flags such as `WantBlood`. The engine accepts both `.zip` and loose
`.gms` scenes under `ScenesPath`, which is exactly the loose-file override this VFS implements.

### Scene ZIP contents

Every `Scenes/**/<name>.ZIP` is an ordinary ZIP (stored or deflated entries) with **one entry per
format** named `Scenes/<dir>/<name>.<EXT>` (upper-case extension, e.g. `Scenes/M01/M01_main.GMS`):
`GMS BUF PRP PRM TEX MAT ANM LOC SND OCT SGP SGD RMC RMI ZGF SUP` (loader scenes have no `LOC`),
plus shared extras that every scene archive carries its own copy of:

* `Shaders/PC/FX/*.fx` (32 DirectX effect files, e.g. `Standard13.fx`, `Skin20.fx`) and `.get_date.dat`
* `menu/*.xml` (UI layouts, differ per scene: `briefing.xml`, `ingame.xml`, `frontend.xml` ...),
  `difficultysettings.xml`, `money.xml`, `notoriety.xml`

## `Vfs` API

```rust
let mut vfs = bm_vfs::Vfs::from_env()?;            // or Vfs::new(path)
vfs.add_mod_dir("mods/my_mod")?;                   // later mods override earlier ones
let gms: Vec<u8>   = vfs.open("scenes/m01/m01_main.gms")?;   // case-insensitive, / or \
let info           = vfs.stat("menu/ingame.xml");            // Option<EntryInfo> (path, Source, size)
let all_tex        = vfs.list("Scenes/**/*.tex");            // glob: * (segment), ** (any depth), ?
vfs.prioritize_archive("Scenes/M01/M01_main.ZIP");           // choose which scene's menu/shader copies win
let raw            = vfs.open_in_archive("Scenes/M00/M00_main.ZIP", "menu/default.xml")?;
let entries        = vfs.list_archive("Scenes/M01/M01_main.ZIP")?;
let on_disk        = vfs.disk_path("pc_eng.str");            // loose files only
```

Resolution order for `open` / `stat` / `list`:

1. mod directories, **last added first** (loose files; also `Scenes/**.zip` placed in a mod dir are mounted
   and searched *after* that mod's loose files);
2. loose files of the game dir, then the scene ZIPs of the game dir (sorted by path; `prioritize_archive`
   moves one to the front);
3. nothing found -> `VfsError::NotFound`.

Path keys are lower-cased with `/` separators (`normalize`). Original casing is preserved in `EntryInfo::path`.
The game dir scan skips `bloodmoney-rs`, `target`, `.git`, `node_modules` (the Rust workspace lives inside the
game folder on the dev machine). A mod can therefore replace e.g. `Scenes/M01/M01_main.TEX` with a loose file
or add `Scenes/Foo/Foo.ZIP`.

ZIP entries are decompressed on every `open` (the `zip` crate; `ZipArchive` handles are kept open behind a
`Mutex`, so `Vfs` is `Sync`).

## Sound banks (`bm_vfs::sound`)

`<scene>.WHD` (directory) + `<scene>.WAV` (data). Layout, field table and confidence levels are documented in
the module docs of `bm-vfs/src/sound.rs`; short version:

* `.WHD`: 16-byte header `(len-16, len, 3, 4)`, then *groups* = `n` NUL-terminated names (aliases, each padded
  to 16 bytes, plus an extra zero block when the NUL lands on a boundary) followed by `n` 52-byte records,
  then 16 zero bytes. 56 864 entries over all shipped banks parse, and the resident entries tile each `.WAV`
  exactly.
* Record: format tag (`0x11` = Microsoft IMA ADPCM, `1` = PCM), flags (`0x80` lip-sync dialogue "PHO",
  `0x180`/`0x8280` music), sample rate, bits, channels, byte size, **data offset**, sample count, block align
  (1024), samples per block (2041).
* Resident entries live in the `.WAV` (16-byte header + back-to-back sample data, no RIFF).
  Streamed entries (`SoundEntry::is_streamed`) live in `pc_eng.str` at `record[8]`.

```rust
let bank = bm_vfs::SoundBank::open(&vfs, "Scenes/M01/M01_main")?;
let i = bank.find("Action/Wipe_Table/Wipe_Table_01.wav").unwrap();
let pcm: Vec<i16> = bank.decode_pcm16(i).unwrap();                 // interleaved
// streamed dialogue:
let stream = bm_vfs::StrIndex::open(&vfs, "pc_eng.str")?;
let pcm = bank.decode_streamed_pcm16(j, &stream)?;
let wav = bm_vfs::sound::pcm16_to_wav(&pcm, e.channels() as u16, e.sample_rate());
```

`decode_ima_adpcm` implements MS IMA ADPCM (mono and stereo). Verified: decoded lengths equal the directory's
sample counts for every resident entry of three banks and for 25 streamed dialogue lines.

## `pc_eng.str` (`bm_vfs::stream`)

* Header (0x100 bytes): `"IOISNDSTREAM"`, `u32 version = 9`, `u32 hash`, `u32 count = 0xd56`,
  `u32 first_chunk = 0x100`, zero padding.
* Chunks start at 0x100. Dialogue chunks begin with `"LIP "` (`u32 n`, `u32 13`, ...: lip-sync/phoneme data,
  occupies the first 0x1000 bytes), followed by IMA ADPCM data. 2717 `LIP ` tags (all 0x100-aligned) exist;
  the header count (3414) also covers chunks without lip data.
* Streamed `.WHD` entries address this file directly: `record[8]` = offset (pointing either at a `LIP ` chunk,
  then audio starts `+0x1000`, or directly at an ADPCM block header), `record[6]` = byte length.
* **Unknown**: how the engine indexes the chunks other than through `.WHD` records; meaning of the two equal
  `u32`s attached to voiced `.LOC` lines (e.g. `0x000be130`, spaced `0x140` apart).

`StrIndex::open_path/open`, `read_at`, `scan_lip_chunks(start, end)`.

## Tests

`cargo test -p bm-vfs` - unit tests plus real-file tests that **skip** (print a note, pass) when
`BM_GAME_DIR`/the default install is missing.
