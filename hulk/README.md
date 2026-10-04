# Hulk model (Blender)

A green muscle giant in torn blue jeans, built procedurally from metaball "muscles" and modelled on a reference screenshot.

| File | What it is |
|---|---|
| `build_hulk.py` | Generator script. Edit the numbers in it to reshape the character. |
| `hulk.blend` | Blender scene with the character, a road plane, a camera and lights |
| `hulk.glb` | The character only, as glTF (Unity, Godot, three.js, Sketchfab) |
| `hulk.obj` / `hulk.mtl` | The character only, as OBJ |
| `hulk_render.png` | Cycles preview render |

Rebuild:

```sh
blender --background --python build_hulk.py -- --render
# or, without a Blender install:
pip install bpy && python build_hulk.py --render
```
