# bm-ui - menus, HUD, fonts, localisation, input

Crate `bm-ui` (depends on `bm-vfs`, `bm-formats`, `bm-render`; wgpu 30). Everything except `gpu.rs` is
renderer-agnostic and unit-tested without a GPU. Everything below was derived from the shipped files
(`menu/*.xml`, `.LOC`, `.PRP`/`.GMS` scene nodes, `.PRM` chunks, `.TEX`) and verified by rendering the real main menu
(`cargo run --release -p bm-ui --example render_ui`, output in `target/ui_out/`).

Confidence legend as in `formats.md`: **H** verified on all shipped files / pixel-checked, **M** verified on samples,
**L** guess, **?** unknown.

## 1. What the game ships (survey)

| data | where | notes |
|---|---|---|
| Menu layout XML | `menu/*.xml` inside **every** scene ZIP | 12 distinct files, each byte-identical across the scenes that carry it (`vfs.md`): `default` (1 window on PC), `frontend` (41 windows, 178 items), `online` (73 / 507, EidosOnline - dead), `saveandc` (7), `briefing` (22), `ingame` (32), `inventor` (1), `postmiss` (7 / 162), `premiss` (8). `frontend/online/saveandc/default` live in `Scenes/HitmanBloodMoney.ZIP` / `saveandcontinue.ZIP`, the others in `hideout_main` and the per-mission `*_premission` / `*_postmission` ZIPs |
| Gameplay tuning XML | `difficultysettings.xml`, `money.xml`, `notoriety.xml` (scene ZIP root) | not UI: per-difficulty AI/hit-point tables (`Rookie/Normal/Expert/Professional` attrs), mission rewards/briefing shop prices, notoriety deltas (`Witness1 value=10`, `BribePolice -40`, ...). Belongs to `bm-game`; plain `<Tag value="N"/>` trees |
| Fonts | `ZFONT` scene nodes (type `0x20004c`) `Text`, `Menu`, `Header` under `AllLevels/fonts.zip/Fonts` | baked TrueType atlases, see 3. Textures `Fonts/Square721_Dm-Normal-1.ttf-14#0..1`, `Font - TrueType - Square 721 Bold Condensed BT.ttf-16#2..3` and `-20#4..8`, all 128x128 RGBA (white glyph + alpha) |
| 2D pictures | `ZWINPIC` nodes (`0x200046`, 598 in the front-end scene, 469 in M01) | multi-tile sprites, see 4 |
| Menu text lines | `ZTEXT` nodes (`0x200038`, "LineObj#NN", 213 in the front-end scene) and `0x20002d` (outlined text of the OSD) | pre-allocated text line objects `{pos, ALIGNMENT_*, DRAWMODE_*, text, font node path, colour[3], width}`; the engine fills them from the XML |
| Localisation | `.LOC` of the scene | `AllLevels/Interface/**` (menu texts: `Frontend`, `Options`, `Buttons`, `Headers`, `Dialog`, `KeyConfiguration`, `Online`, ...), `AllLevels/Info` (in-game info/hints), `AllLevels/Actions`, `AllLevels/Items`, `AllLevels/Oneliners` (subtitles), `NewsPaper/**`, `M01/..` level strings. 4626 leaves in the front-end scene |
| In-game HUD ("OSD") | scene group `AllLevels/mainsceneincludes.zip/AllLevels/osd.zip` (in every mission scene) | `Compass` (N/E/S/W + `CompassMarker`), `WeaponDisplay` (`FrameLarge`, `BulletInfo`, `Text` + outlines), `InfoDisplay` (`TickoffMark`, `Symbol`, `Text`), `ActionMenu` / `PickupMenu` / `DropDisplay` / `ContainerMenu` (`FocusBar`, `FocusBarMore`, `ButtonSymbol`, `BigButtonSymbol_PcOnly`), `SubTitles` (`SubTitles` + `SubTitles_Outline`, ZTEXT), `ItemWiever` (3D weapon preview with lights/camera `OsdCamera`/`ItemViewCamera`), `CheatMenu`; also a copy of the menu resources (`xmlmenuresources.zip/SkipSave/ZWINDOWS/...`: `Mouse`, `Menu`, `TRC`, `Dialog`, `Background`, Resources/{Frames,LineObjs,Sliders,ButtonGraphic,Graphic}) |
| Menu 3D hosts | `ZWINDOWS` (`0x100030`), `Menu` (`0x100031`), `DefaultFocus` (`0x100033`), `Control#nn` (`0x80100032`), `Button#nn` (`0x100033`) + one `ZCAMERA` per layer (`MouseCam`, `TRCCam`, `DialogCam`, `BackgroundCam`, `OsdCamera`) | the engine renders each layer with an orthographic-ish camera (FOV 45, `CAMERA_ASPECT_NORMAL`) |
| Bindings / config | `HitmanBloodMoney.ini` | only `[MOUSE] UseMouse/MouseSpeed 0.4`, `[GAMEPLAY] Subtitles/PipCam/AutoAim/AutoCrouchWhenTighteningFiberWire`, resolution etc. The controller/keyboard file `HM3GPAD.cfg` is **not shipped**: bindings are baked into the exe (our defaults are our own). The action ids are visible as LOC keys `Interface/KeyConfiguration/*` and the `ZW_*` button ids in the XML |
| Images | `Bitmaps/Screens/MainMenu/HMBMLogo_01` (512x256 RGBA logo), `LogoReflection`, `Bitmaps/OSD/Pointers_01_OSD_H3`, `Bitmaps/InGameMap/Legend_01_BIT_H3`, `Bitmaps/Screens/MapOverlay/*` | everything else the UI draws is small tile textures (`8x8`, `512x8`, ...) referenced only by `ZWINPIC` chunks |

Default scene: `DefaultScene=M01/M01_main.gms` in the shipped ini (the original shipped ini starts the boot menu;
the front-end scene is `Scenes/HitmanBloodMoney.ZIP`).

## 2. Menu XML (`menu/*.xml`) [H syntax, M semantics]

Files are not well-formed XML (3 of 12 fail a strict parser: duplicate attributes like
`<Template Name="PrevButton" Name="LeftArrow">`, raw `<` inside `Text="<fix to dynamic name here>"`), Latin-1 or UTF-8.
`bm_ui::xml` is a tolerant parser (first duplicate wins + warning).

Vocabulary (counts over all 12 files):

* `Windows` root. `ColorSetDefinition Name NormalColor FocusColor [DisableColor]` - colours as `0xAARRGGBB` **or**
  `r,g,b,a`. `ButtonGraphicDefinition Name [GraphicsOnly TextOffsetX/Y TextAlignment H] { GraphicElement Name AnimateAlpha
  Priority X Y { Type Value=Focus|Normal|Disabled } }` (19). `Template Name ..attrs.. [children]` (229): attribute bundle,
  may include `Template=` itself (chains up to 3 deep) and may hold child `GuiElement`s (group templates `Header`, `Legend*`,
  `Top`). `Window Name Template [PopOnBack Background Frame Dialog TRC RollBackMark Navigation PauseEngine ..]` (102).
* `Platform PC|XBOX|XENON|PS2="True"` (150) and `MainIniOption Name Value` (conditional blocks, e.g. `DebugMenus`).
  PC window template: `W=1024 H=768 AlignH/AlignV=Center LineSpace=11`.
* `GuiElement` (762): `Type` (49 distinct values), `Text` (LOC key relative to `AllLevels/Interface/`, `%literal` for
  debug), `X Y Width Alignment Font(Menu|Header|Text) ColorSet Link ReceiveAllInput ActivateButton(ZW_SELECT4..) TakeFocus
  Window Scene Option LowerBound UpperBound Step SliderSize ButtonGraphic Graphic ID ActionMessage OnRemoval ...`.
  Types by use: `TextBlock` 163, `OpenWindow` 86, `CloseWindow` 56, `OnMCChange` 21 (memory card), `Graphic` 21,
  `CheckForNonConcealableWeapons` 16, `ZLevelPerformanceNoise/Violence` 20, `Slider`, `HM3ToggleOption` (+ `ToggleTextItem`
  children), `CheckBox`, `GUIList` (+ `ItemLine SizeY`), `SaveGame/LoadGame/SaveProfile/...`, `LoadScene*`, `StartMission`,
  `BuyUpgrade`, `WeaponSelector`, `Inventory`, `ZMissionBreakDown`, `ZBribesNavigator`, `ZGuiNoterietyIndicator`,
  `ProfileName`, `QuitLevel`, ... (online/Xbox-live types are kept as `ItemKind::Other` / `Action::Custom`).
* `<Navigation Navigation="Vertical|Horizontal">` (49) groups elements into a focus group (button legends, accept/cancel pairs).

`bm_ui::menu::MenuDoc::parse_*` resolves templates + conditionals into `Window { items: [Item] }`, mapping `Type` to
`ItemKind` (`Text, Header, Button(Action), Slider, Toggle, CheckBox, Graphic, List, Group, Other`) and `Action`
(`OpenWindow, CloseWindow, LoadScene, Quit, SetDifficulty, Custom{kind}`). The raw resolved attributes stay in `Item::attrs`.

### Layout rules (`menu_view`) [M]

Window rect = `W x H` centred on the 1024x768 design canvas (`AlignH/V Center`); `X/Y` are offsets from the window's top-left;
`Alignment` says what `X` means (Left edge / centre / Right edge - `LeftPosX` is `X=376 Alignment=Right`: labels end at 376);
items without `Y` flow below the previous one (`+ LineSpace`); members of a horizontal `Navigation` group share the cursor;
toggles draw the label at `X` and the choice at `X + OptionOffset`; sliders put the track at `X + SliderOffset` with
`SliderSize` pixels and the label at `X + TextOffsetX` (from the `ButtonGraphic`). Verified visually against the original
options screens' structure; exact pixel metrics (focus bar art, slider artwork, paddings) are **L**: we draw procedural
highlights instead of the `FocusBar`/`MenuOverlay` sprites whose use-sites are in native code.

Canvas mapping to the screen: `draw::Viewport::new(sw, sh, 1024, 768)` (uniform scale, centred, bars on the rest) -
the original is 4:3 only. HUD and mod screens use anchors on the real screen instead.

## 3. Fonts [H glyph layout, M kerning, ? page link]

`ZFONT` node props tail: `ALIGNMENT_None, DRAWMODE_OPACITY, TYPE_Normal, I32 prm_chunk, I32 font_class`. The PRM chunk:

```text
u32 version = 1, u32 8 (?), u32 pixel size (14/16/20), f32 1.0, u32 glyph_count (209/257/257), u32 line_height (15/..),
u32 kerning_count (45/..), u32 page_count (2/2/5)
glyph_count x { u32 codepoint, u32 page_chunk, bytes x0 y0 x1 y1, bytes w h xoff(i8) yoff(i8), bytes advance line 0 0xcd }
kerning_count x { u32 pair = second<<16 | first (direction L), i32 delta }  + 1 trailing word
```

Codepoints are Unicode (ASCII, Latin-1, a few typographic ones, `0xF001/0xF002` private-use button icons). `yoff` is relative to
the baseline (`'!'` is 11 high with `yoff=-10`), advance in pixels. Verified by drawing: every glyph rectangle lies inside its
128x128 page and the rendered "START GAME / OPTIONS / CREDITS" is pixel-clean. **Page link:** `page_chunk` references a
64-byte descriptor chunk (`[0x80089, .., 0x80 (=128), .., chunk, ..]`) whose chain to the TEX image is not decoded; we map a font's
distinct page chunks (ascending) onto its `Fonts/..#n` TEX images in order (offset by the pages of the previous fonts) - correct
for all three fonts (rendered).

LOC markup used by texts (counted in the front-end LOC): `<br>` 499, `<tab>`, `<font N>` (N = 2,3,4: newspaper fonts, mapping to the
three baked fonts is **L**), `<color rrggbb>`, `<caps next|word>`, `<justify fill>`, `<width N>`, `<kerning N>`,
`<linespacing N>`, `<tabsize N>`, `<img src= align=>`, `<autocolumn>`/`<gutter>` (newspaper layouter only), `{locale}key{endlocale}`
(1767; nested key reference), `{if Var more|less|equal|not equal N}..{else}..{endif}`, `{Var}` statistics placeholders
(`{TotalKilled}`, `{CurrentLevel}`), button glyph tokens `{ZW_SELECT}` `{ZW_CANCEL}` `{Start}` and `{bullet_square}`.
`bm_ui::strings` expands locale/if/var; `bm_ui::text` handles the layout tags; unresolved `{ZW_*}` tokens are replaced with the
current binding label (`[Esc]`) by `UiContext::token`.

Mod fonts: `Fonts/<Name>.fnt` (BMFont *text* format) + PNG pages, same-name replace; `BitmapFont::to_fnt_text` dumps a game font.

## 4. ZWINPIC sprites [H]

Node props tail: `Bool, I8(-1), I8 layer, ALIGNMENT_<Top_Left|Left|Center..>, DRAWMODE_<OPACITY|ADD|NORMAL>, TYPE_<Background|Normal>,
I32 width, I32 height, I32 prm_chunk`. The chunk (`sprite::parse_winpic_chunk`):

```text
u32 n,  n x { f32 cx, cy (tile centre, y up, relative to the picture's top-left), f32 u0 v0 u1 v1, 0, f32 w h, u32 argb },
u32 n,  n x chunk ref -> 32-byte image chunk [0x20101, tex_id, 0, 5, flags, 0,0,0]  (tex_id is a plain TEX image id)
```

Pictures are cut into power-of-two tiles (the 9x11 `ScrollDown` arrow is four tiles 8x8, 4x8, 8x3, 4x3 using ids 530..533);
`u1/v1 < 1` mark partly used textures (e.g. 448 of 512). `HitmanBackground` is 1024x768 from 512-px tiles. `DRAWMODE_ADD` =
additive glow (`MouseBackLight`); `OPACITY`/`NORMAL` are drawn with alpha blending (NORMAL = no blending is **L**).
Named lookup (`SceneUi::sprite("HitmanBackground")`): a `ZWINPIC` node name or a group whose child picture is used (`A/B`
picks child B). Which node an XML `Graphic=`/`GraphicElement Name=` picks (`ScrollUp#01` = Focus, `#02` = Normal ...) is by name
only; the binding of `ButtonGraphic` states to nodes is **?**.

## 5. HUD [M]

The OSD groups (above) are the *art/layout slots*; the values (health, ammo, notoriety, suspicion), the compass heading and
the subtitle timing come from native controllers that `bm-game` replaces. `bm_ui::hud` therefore models the HUD as data:

* `HudLayout` (RON/JSON): widgets with an anchor (`TopLeft..BottomRight`), offset/size in reference pixels (720p), optional
  visibility expression (`has_weapon`, `suspicion>0`) and a kind: `Panel`, `Bar` (value/max variables, direction, segments,
  low-value colour + pulse, label), `Text` (`{var}` interpolation, LOC keys or `%literal`, font, alignment, shadow/outline,
  wrap), `Icon` (UI-context sprite), `Subtitles`, `Prompts` (`[E] Open door`, key label from the bindings), `Compass`.
* `HudState`: named `f32` variables, named strings, a subtitle queue (`say(speaker, text, seconds)`, fade in/out) and prompt list.
* Built-in layout = `hud::DEFAULT_LAYOUT_RON` (compass, health, notoriety segments, suspicion, weapon + ammo, prompts, subtitles).
  Variables the game layer should set: `health`, `health_max`, `notoriety`, `notoriety_max`, `suspicion`, `suspicion_max`,
  `has_weapon`, `weapon_name`, `ammo_clip`, `ammo_reserve`, `heading` (deg, 0 = north), `show_compass`.

Voiced lines use `Interface/../Oneliners/*` LOC leaves with `pc_eng.str` stream words (`vfs.md`); subtitles take any LOC key.

## 6. Input [M names, L defaults]

`input::Action` mirrors the engine ids (`MoveUp/Down/Left/Right, Run, CrouchSneak, FireWeapon, FireWeapon2 (scope), ReloadWeapon,
UseAction, ActionMenuOpen/Fast, PickupMenuOpen/Fast/SPC, InventoryMenuOpen, DropItem, DropVideo, Holster, Cycle*, ToggleCamera,
CameraZoomIn/Out, Map, Briefing, IngameMenu` and menu `ZW_UP/DOWN/LEFT/RIGHT/SELECT/SELECT2/SELECT4/CANCEL`, `Accept`, `Enter`);
display names are the LOC keys `KeyConfiguration/<Action>`. `Binding` = `Key("KeyW")` (winit `KeyCode` names) | `Mouse(..)` | `WheelUp/Down` |
`Pad(..)` | `PadAxis(axis, dir)`; up to 3 slots per action. `InputMap` serialises to RON (default) or JSON in
`%APPDATA%/bm-rs/input.ron` (`input::user_config_path()`), tolerates partial files (missing actions get defaults), reports
conflicts on `rebind`. `InputState` turns app `InputEvent`s into `down/pressed/released/value/move_vector` per action and supports
"press a key to bind" capture. Mod defaults: `Config/input.ron`.

## 7. Modding hooks (all through the VFS, loose files win) [H, tested]

| file | effect | hot reload |
|---|---|---|
| `menu/<name>.xml` | replace a shipped menu document | yes (`UiSystem::poll_reload` -> `Reloaded::Menu`) |
| `menu/<name>.add.xml` | merge extra/replaced windows & styles onto it | yes |
| `Fonts/<Name>.fnt` + PNG | add/replace a bitmap font | yes |
| `Localisation/<lang>.json|ron`, `Localisation/<lang>/*` | flat string tables (relative or absolute keys), fallback `en` | yes |
| `UI/hud.ron|json` | HUD layout | yes (broken file -> builtin layout) |
| `UI/Screens/*.ron|json`, `UI/Images/*.png` | new screens (panels, text, images, buttons -> `Open/Back/Event`) | yes |
| `Config/input.ron` | default bindings | restart |
| `Shaders/WGSL/ui.wgsl` | UI shader (bindings documented in the file) | `UiRenderer::poll_hot_reload` (broken edit keeps the old shader) |

`bm-vfs` indexes loose files at start-up: edits to existing files are picked up, **new** files need a restart (same limitation as
the shader hot reload in `render.md`).

## 8. Rendering [H]

`draw::DrawList` batches quads (texture, blend `Alpha|Additive`, scissor) in screen pixels with a pushable uniform transform;
`gpu::UiRenderer::render(encoder, &color_view, size, &list, &store)` records a pass with `LoadOp::Load` after `bm_render::Renderer::render`
(same device/queue/format via `UiRenderer::for_renderer`); textures come from `TextureStore` and are uploaded lazily /
re-uploaded on `replace`. For sRGB targets the shader (override constant `SRGB_TARGET`) converts authored sRGB colours to linear,
so colours come back unchanged (pixel-tested). `UiOffscreen::render_rgba(.., Some(Scene3d{..}), ..)` renders a 3D frame, then the UI.

## 9. Open questions

* Link from a font page descriptor chunk (`0x80089`...) to its TEX image; meaning of font header word 1 (`8`) and the kerning pair direction.
* Which `ButtonGraphic` state maps to which `ZWINPIC` node, and the native layout of focus bars / sliders (we use procedural art).
* `ZTEXT` (`0x200038`/`0x20002d`) property tail beyond `{text, font path, colour, width}`; `ZWINDOWS/Menu/Control#nn` (`0x80100032`) payload.
* Exact HUD placement/art of the original OSD (native controllers); `<font N>` numbering; `DRAWMODE_NORMAL` blending.
* Original default key/pad bindings (baked into the exe).
