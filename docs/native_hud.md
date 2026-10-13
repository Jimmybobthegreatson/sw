# Native HUD (bm-bevy `native_hud.rs`)

Everything below was read from `HitmanBloodMoney.exe` with Ghidra (scripts in `C:\Users\Shadow\hitman-re\scripts`, project
`hitman-re\proj`): `ZOSD` vtable 0x79ffe4, `ZWeaponDisplay` 0x7a166c, `ZInfoDisplay` 0x7a18bc.

| What | Function | Finding |
|---|---|---|
| Layout of all OSD elements | `ZOSD::Layout` 0x6614e0 | health frame at `(48, H-101)`, weapon display at `(W-212, H-99)`, compass scale at `x = W-129.5`, letters 32 px apart from `W-96`, marker at `W-100`, pointer at the screen centre - 16 |
| Draw order | `ZOSD::Draw` 0x662490 | health/stealth bars, pointer, weapon display, compass, subtitles, info display |
| Stealth bar colour | 0x6601d0 | value <= 50: `#00ae09`, <= 75: `#e9c616`, above: `#da0000`; the bar height is `value * 0.009` of 38 px |
| Compass scroll | 0x661a20 | `shift = heading / 360 * 128`, letters wrap every 128 px, fade over 32..64 px from the centre |
| Weapon display | 0x66e790 | `FrameLarge` at `(x-16, y-2)`, bullet line 25 px below the name line |
| Art | `.data` table 0x806220 | `ui_health_stealth_bars#health_bar / tricolor_bar_01`, `ui_compass#CompassScale`, `UI_all_weapons#*_costum` (M4, MP5, pistol, WA2000, SPAS12), `Pointers_01_OSD_H3` |

## Key hints (G / SPC / E) - what was traced and what was not

The three hints are the `DropDisplay`, `ActionMenu` and `PickupMenu` window groups of the OSD scene (key symbol `ButtonSymbol` +
text `Line1`). Their draw functions are 0x669610 (`FUN_00669610`, symbol 0x34 + outline 0x88/0x8c + text lines 0x40/0x44) and
0x66b130; the row layout is `0x66acc0` (text x = origin + 20 + 32, outline at origin, second line 4 px right of the origin).
ZOSD hands each menu an anchor `(-200, -100)` (fields 0x1b0 / 0x1c4 / 0x1d8 / 0x1ec) in OSD-camera units; the conversion of
that anchor to screen pixels (scale at ZOSD+0x8c8) was not resolved, so `native_hud.rs` still places G / SPC / E from the
reference footage.

## Item panel (measured on the original at 800x600, opened by holding the right mouse button)
Panel x 215..585 (370 wide); header y 426..448 (grey 107 -> 82, 1 px highlight above, 63 line below); description area y 448..548 (near-black); footer y 548..570 (grey 39); arrows 13x16 px at 16 px outside both ends; name at x+23, description at x+14, y+6. The background is blurred and darkened in the original (only darkened here).
