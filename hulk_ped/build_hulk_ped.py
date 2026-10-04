#!/usr/bin/env python3
"""Build a Hulk as a GTA V ped (Sollumz-ready) from scratch.

    python build_hulk_ped.py --sollumz /path/to/Sollumz [--quick]

Needs: Blender 4.2+ (or ``pip install bpy``), numpy, scipy, scikit-image, pillow, xatlas, szio and the Sollumz
add-on (https://github.com/Sollumz/Sollumz).  Everything lands in ``out/``:

    out/hulk_ped.blend      Sollumz scene: 6 ped components on the stock GTA V ped skeleton
    out/gta5/*.ydd.xml      CodeWalker XML drawables (head, hair, uppr, hand, lowr, feet) + embedded textures
    out/gta5/*.ytd.xml      diffuse texture dictionaries
    out/textures/*.dds      DXT-compressed textures
    out/preview/*           renders and the glTF used by the web viewer
"""
import argparse
import math
import os
import pickle
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

ap = argparse.ArgumentParser()
ap.add_argument("--sollumz", default=None, help="folder of the Sollumz add-on checkout (if not installed in Blender)")
ap.add_argument("--out", default=os.path.join(HERE, "out"))
ap.add_argument("--voxel", type=float, default=0.0025, help="sculpt resolution in metres (smaller = finer, slower)")
ap.add_argument("--quick", action="store_true", help="coarse sculpt + small textures for fast iteration")
ap.add_argument("--no-export", action="store_true")
ap.add_argument("--no-preview", action="store_true")
args = ap.parse_args()
if args.quick:
    args.voxel = max(args.voxel, 0.004)

from lib.sollumz_env import prepare_addon_path  # noqa: E402  (must run before importing bpy)

prepare_addon_path(args.sollumz)

import numpy as np  # noqa: E402
import bpy  # noqa: E402

from lib import anatomy, bake, dds, eyes, meshing, shading, skinning, uvmap  # noqa: E402
from lib import ped_rig, sollumz_build  # noqa: E402
from lib.meshing import Parts  # noqa: E402
from lib.sdf import Grid  # noqa: E402
from lib.skeleton import Joints  # noqa: E402
from lib.sollumz_env import enable_sollumz  # noqa: E402

T0 = time.time()


def log(msg):
    print(f"[{time.time() - T0:6.1f}s] {msg}", flush=True)


OUT = os.path.abspath(args.out)
TEX_DIR = os.path.join(OUT, "textures")
GTA_DIR = os.path.join(OUT, "gta5")
PREV_DIR = os.path.join(OUT, "preview")
for d in (OUT, TEX_DIR, GTA_DIR, PREV_DIR):
    os.makedirs(d, exist_ok=True)

# texture size per component (power of two)
RES = {"head": 1024, "uppr": 2048, "hand": 512, "feet": 1024, "lowr": 2048, "hair": 512}
TRIS = {"skin": 19500, "jeans": 6500, "hair": 2200}
if args.quick:
    RES = {k: max(256, v // 2) for k, v in RES.items()}

bpy.ops.wm.read_factory_settings(use_empty=True)
enable_sollumz()
from Sollumz.sollumz_properties import LODLevel  # noqa: E402

J = Joints()
R = anatomy.Rig(J)

# ---------------------------------------------------------------------------------------------------------------------
# 1. sculpt -> game-resolution parts
# ---------------------------------------------------------------------------------------------------------------------
log(f"sculpting at {args.voxel * 1000:.1f} mm voxels")
grids = {}
parts = {}   # name -> (verts, tris)

g = anatomy.new_grid(args.voxel)
anatomy.build_skin(g, R)
grids["skin"] = g
v, f = g.mesh()
log(f"skin sculpt: {len(f):,} triangles")
dv, df = meshing.decimate(v, f, TRIS["skin"], meshing.density_weights_skin(v, R))
dv, df = meshing.cleanup(g, dv, df)
lab = meshing.classify_skin(meshing.face_centroids(dv, df), R)
for pid, nm in ((Parts.UPPR, "uppr"), (Parts.HEAD, "head"), (Parts.HAND, "hand"), (Parts.FEET, "feet")):
    sv, sf, _ = meshing.submesh(dv, df, lab == pid)
    parts[nm] = (sv, sf)
del v, f

gj = anatomy.new_grid(args.voxel)
anatomy.build_jeans_full(gj, R)
grids["jeans"] = gj
jv, jf = gj.mesh()
jd, jdf = meshing.decimate(jv, jf, TRIS["jeans"], meshing.density_weights_jeans(jv))
jd, jdf = meshing.cleanup(gj, jd, jdf)
parts["lowr"] = meshing.cut_jeans(jd, jdf)[:2]
del jv, jf

gh = anatomy.new_grid(args.voxel)
anatomy.build_hair_full(gh, R)
grids["hair"] = gh
hv, hf = gh.mesh()
hd, hdf = meshing.decimate(hv, hf, TRIS["hair"])
hd, hdf = meshing.cleanup(gh, hd, hdf)
parts["hair"] = meshing.cut_hair(hd, hdf)
del hv, hf
log("parts: " + ", ".join(f"{k}={len(t[1]):,}" for k, t in parts.items()))

GRID_OF = {"head": "skin", "uppr": "skin", "hand": "skin", "feet": "skin", "lowr": "jeans", "hair": "hair"}
KIND_OF = {"head": "head", "uppr": "body", "hand": "body", "feet": "body", "lowr": "body", "hair": "hair"}

# ---------------------------------------------------------------------------------------------------------------------
# 2. unwrap + bake textures + LODs
# ---------------------------------------------------------------------------------------------------------------------
noise = bake.Noise3(5)
data = {}
for name, (pv, pf) in parts.items():
    grid = grids[GRID_OF[name]]
    res = RES[name]
    vmap, nf, uv, st = uvmap.unwrap(pv, pf, lambda p, _g=grid: _g.gradient(p, eps=0.014), resolution=res, padding=4)
    nv = pv[vmap]
    vn = grid.gradient(nv, eps=0.010)
    pm = bake.PartMaps(nv, nf, uv, vn, res)
    pm.project(grid)
    ao = bake.ambient_occlusion(grid, pm.spos, pm.snrm)
    if name == "lowr":
        col, spec = shading.denim(pm.spos, pm.snrm, ao, noise)
    elif name == "hair":
        col, spec = shading.hair(pm.spos, pm.snrm, ao, noise)
    else:
        col, spec = shading.skin(pm.spos, pm.snrm, ao, noise, name)
    dx, dy = bake.micro_detail(res, seed=hash(name) % 1000)
    strength = {"lowr": 0.16, "hair": 0.0}.get(name, 0.10)
    nmap = bake.normal_map(pm, grid, detail=(dx[pm.mask], dy[pm.mask]), detail_strength=strength)
    data[name] = dict(
        verts=nv, tris=nf, uv=uv, diffuse=pm.to_image(col), normal=nmap, spec=pm.to_image(spec),
        lods={LODLevel.HIGH: (nv, nf, uv)})
    n0 = len(nf)
    for level, frac in ((LODLevel.MEDIUM, 0.5), (LODLevel.LOW, 0.22), (LODLevel.VERYLOW, 0.08)):
        lv, lf, luv = meshing.decimate_uv(nv, nf, uv, max(60, int(n0 * frac)))
        data[name]["lods"][level] = (lv, lf, luv)
    log(f"{name}: {n0:,} tris, {len(nv):,} verts, {st['islands']} UV islands, "
        f"LODs {[len(x[1]) for x in data[name]['lods'].values()]}")

# eyes
eye_parts = []
for sx in (1, -1):
    c = np.array([sx * R.eye[0], R.eye[1] + 0.0008, R.eye[2]])
    ev, et, euv, en = eyes.eyeball_mesh(c)
    nrm_out = np.cross(ev[et[:, 1]] - ev[et[:, 0]], ev[et[:, 2]] - ev[et[:, 0]])
    cent = ev[et].mean(1) - c
    if (nrm_out * cent).sum() < 0:
        et = et[:, [0, 2, 1]]
    eye_parts.append((ev, et, euv, en))

# ---------------------------------------------------------------------------------------------------------------------
# 3. textures -> DDS
# ---------------------------------------------------------------------------------------------------------------------
TEX = {}   # logical name -> path


def to_dx(normal_rgb):
    n = normal_rgb.copy()
    n[..., 1] = 1.0 - n[..., 1]   # GTA V uses DirectX-style normal maps
    return n


def write_tex(name, rgb, alpha=False):
    img = np.concatenate([rgb, np.ones(rgb.shape[:2] + (1,), np.float32)], axis=-1)
    path = os.path.join(TEX_DIR, name + ".dds")
    dds.write_dds(path, img, alpha=alpha)
    return path


for name, d in data.items():
    TEX[name + "_diff"] = write_tex(f"{name}_diff_000_a_uni", d["diffuse"])
    TEX[name + "_normal"] = write_tex(f"{name}_normal_000", to_dx(d["normal"]), alpha=True)
    TEX[name + "_spec"] = write_tex(f"{name}_spec_000", d["spec"])
TEX["eyes_diff"] = write_tex("eyes_diff_000_a_uni", eyes.eye_texture())
log("textures written")

# ---------------------------------------------------------------------------------------------------------------------
# 4. Sollumz scene
# ---------------------------------------------------------------------------------------------------------------------
coll = bpy.data.collections.new("Hulk_Ped")
bpy.context.scene.collection.children.link(coll)
bpy.context.view_layer.active_layer_collection = bpy.context.view_layer.layer_collection.children["Hulk_Ped"]


def load_tex(key):
    return bpy.data.images.load(TEX[key], check_existing=True)


SKIN_PARAMS = {"specularIntensityMult": (0.3,), "specularFalloffMult": (100.0,), "specularFresnel": (0.97,)}
MATS = {}
for name in ("head", "uppr", "hand", "feet"):
    MATS[name] = sollumz_build.ped_material(f"{name}_skin", "ped.sps", load_tex(name + "_diff"), load_tex(name + "_normal"),
                                            load_tex(name + "_spec"), SKIN_PARAMS)
MATS["lowr"] = sollumz_build.ped_material("lowr_jeans", "ped_default.sps", load_tex("lowr_diff"), load_tex("lowr_normal"),
                                          load_tex("lowr_spec"), {"specularIntensityMult": (0.12,), "specularFalloffMult": (60.0,)})
MATS["hair"] = sollumz_build.ped_material("hair_black", "ped_default.sps", load_tex("hair_diff"), load_tex("hair_normal"),
                                          load_tex("hair_spec"), {"specularIntensityMult": (0.3,), "specularFalloffMult": (80.0,)})
MATS["eyes"] = sollumz_build.ped_material("head_eyes", "ped_default.sps", load_tex("eyes_diff"), None, None,
                                          {"specularIntensityMult": (0.6,), "specularFalloffMult": (300.0,)})

COMPONENTS = {   # drawable / file name -> data key
    "head_000_r": "head", "hair_000_u": "hair", "uppr_000_r": "uppr",
    "hand_000_r": "hand", "lowr_000_r": "lowr", "feet_000_r": "feet",
}

master = ped_rig.create_armature("head_000_r")
master.name = "head_000_r"
dict_objs = {"head_000_r": master}
for comp in COMPONENTS:
    if comp not in dict_objs:
        dict_objs[comp] = ped_rig.share_armature(comp, master)
for o in dict_objs.values():
    o.hide_render = True
    o.data.display_type = "STICK"
log(f"armature: {len(master.data.bones)} bones")

bone_names_all = None
for comp, key in COMPONENTS.items():
    d = data[key]
    lod_meshes, lod_weights = {}, {}
    group_names = None
    for level, (lv, lf, luv) in d["lods"].items():
        grid = grids[GRID_OF[key]]
        ln = grid.gradient(lv, eps=0.010)
        mat_idx = np.zeros(len(lf), np.int32)
        verts, tris, uvs, norms = lv, lf, luv, ln
        names, wt, bi = skinning.compute_weights(lv, lf, J, kind=KIND_OF[key])
        group_names = list(names) + (["FACIAL_L_eyeball", "FACIAL_R_eyeball"] if key == "head" else [])
        if key == "head" and level != LODLevel.VERYLOW:
            for k, (ev, et, euv, en) in enumerate(eye_parts):
                off = len(verts)
                verts = np.concatenate([verts, ev])
                tris = np.concatenate([tris, et + off])
                uvs = np.concatenate([uvs, euv])
                norms = np.concatenate([norms, en])
                mat_idx = np.concatenate([mat_idx, np.ones(len(et), np.int32)])
                ew = np.zeros((len(ev), 4), np.float32)
                ew[:, 0] = 1.0
                ei = np.zeros((len(ev), 4), np.int32)
                ei[:, 0] = len(names) + k
                wt = np.concatenate([wt, ew])
                bi = np.concatenate([bi, ei])
        me = sollumz_build.mesh_from_part(f"{comp}_{level.name.lower()}", verts, tris, uvs, norms, mat_idx)
        lod_meshes[level] = me
        lod_weights[level] = (wt, bi)
    mats = [MATS[key]] + ([MATS["eyes"]] if key == "head" else [])
    sollumz_build.build_component(dict_objs[comp], comp, lod_meshes, mats, group_names, lod_weights, dict_objs[comp])
    log(f"component {comp}: LOD tris " + str({k.name: len(m.polygons) for k, m in lod_meshes.items()}))

# texture dictionaries (diffuse maps travel in their own .ytd, normal / specular maps are embedded in the .ydd)
for key in list(data) + ["eyes"]:
    txd = bpy.context.scene.sz_txds.new_texture_dictionary(f"{key}_diff_000_a_uni" if key != "eyes" else "eyes_diff_000_a_uni")
    txd.new_texture(load_tex(key + "_diff"))

for o in dict_objs.values():
    for c in list(o.users_collection):
        c.objects.unlink(o)
    coll.objects.link(o)

blend_path = os.path.join(OUT, "hulk_ped.blend")
bpy.ops.wm.save_as_mainfile(filepath=blend_path)
log(f"saved {blend_path}")

# ---------------------------------------------------------------------------------------------------------------------
# 5. export GTA V assets (CodeWalker XML)
# ---------------------------------------------------------------------------------------------------------------------
if not args.no_export:
    bpy.ops.object.select_all(action="DESELECT")
    for o in dict_objs.values():
        o.select_set(True)
    bpy.context.view_layer.objects.active = master
    r = bpy.ops.sollumz.export_assets(
        directory=GTA_DIR, direct_export=True, use_custom_settings=True,
        target_formats={"CWXML"}, target_versions={"GEN8"}, limit_to_selected=True, exclude_skeleton=True,
        export_ytds=True, export_ytds_include="ALL")
    log(f"export: {r}; files: {sorted(os.listdir(GTA_DIR))}")

# ---------------------------------------------------------------------------------------------------------------------
# 6. showcase renders + glTF for the web viewer (preview copies, the Sollumz scene itself is left untouched)
# ---------------------------------------------------------------------------------------------------------------------
def preview_materials(max_size=1024):
    from PIL import Image

    tdir = os.path.join(PREV_DIR, "tex")
    os.makedirs(tdir, exist_ok=True)
    mats = {}
    for key, d in list(data.items()) + [("eyes", None)]:
        if d is None:
            rgb, nrm = eyes.eye_texture(), None
        else:
            rgb, nrm = d["diffuse"], d["normal"]
        paths = []
        for tag, arr, nc in (("d", rgb, False), ("n", nrm, True)):
            if arr is None:
                paths.append(None)
                continue
            im = Image.fromarray((np.clip(arr, 0, 1) * 255).astype(np.uint8))
            if im.size[0] > max_size:
                im = im.resize((max_size, max_size), Image.LANCZOS)
            pth = os.path.join(tdir, f"{key}_{tag}.png")
            im.save(pth)
            paths.append((pth, nc))
        m = bpy.data.materials.new(f"prev_{key}")
        bsdf = m.node_tree.nodes["Principled BSDF"]
        t1 = m.node_tree.nodes.new("ShaderNodeTexImage")
        t1.image = bpy.data.images.load(paths[0][0])
        m.node_tree.links.new(t1.outputs["Color"], bsdf.inputs["Base Color"])
        if paths[1]:
            t2 = m.node_tree.nodes.new("ShaderNodeTexImage")
            t2.image = bpy.data.images.load(paths[1][0])
            t2.image.colorspace_settings.name = "Non-Color"
            nm = m.node_tree.nodes.new("ShaderNodeNormalMap")
            m.node_tree.links.new(t2.outputs["Color"], nm.inputs["Color"])
            m.node_tree.links.new(nm.outputs["Normal"], bsdf.inputs["Normal"])
        bsdf.inputs["Roughness"].default_value = 0.85 if key == "lowr" else (0.35 if key == "eyes" else 0.5)
        mats[key] = m
    return mats


def preview_copies(prefix, mats):
    """Static copies of the (possibly posed) HIGH-LOD models with simple Principled materials."""
    dg = bpy.context.evaluated_depsgraph_get()
    out = []
    for comp, key in COMPONENTS.items():
        drawable = [c for c in dict_objs[comp].children if c.name.startswith(comp)][0]
        model = drawable.children[0]
        me = bpy.data.meshes.new_from_object(model.evaluated_get(dg))
        me.materials.clear()
        me.materials.append(mats[key])
        if key == "head":
            me.materials.append(mats["eyes"])
        ob = bpy.data.objects.new(f"{prefix}_{comp}", me)
        bpy.context.scene.collection.objects.link(ob)
        out.append(ob)
    return out


def export_glb(objs, path):
    bpy.ops.object.select_all(action="DESELECT")
    for o in objs:
        o.select_set(True)
    bpy.context.view_layer.objects.active = objs[0]
    bpy.ops.export_scene.gltf(filepath=path, export_format="GLB", use_selection=True, export_apply=False,
                              export_image_format="JPEG", export_jpeg_quality=88, export_yup=True)


if not args.no_preview:
    from lib import posing
    from lib.preview import flat_mat, mesh_from_arrays, render_montage

    pmats = preview_materials()
    gnd = bpy.data.objects.new("floor", mesh_from_arrays("floor", np.array([[-4, -4, -1.0], [4, -4, -1.0], [4, 4, -1.0], [-4, 4, -1.0]], np.float32),
                                                         np.array([[0, 1, 2], [0, 2, 3]], np.int32)))
    bpy.context.scene.collection.objects.link(gnd)
    gnd.data.materials.append(flat_mat("floor", (0.30, 0.30, 0.32), 0.9))
    for o in dict_objs.values():
        o.hide_viewport = False
    # (a) rest pose
    rest = preview_copies("rest", pmats)
    for o in dict_objs.values():
        o.hide_set(True)
    render_montage(rest, os.path.join(PREV_DIR, "hulk_ped_rest.png"), views=["front", "three_quarter", "side", "back"],
                   focus=(0, 0, -0.1), dist=4.8, samples=48, size=(560, 880))
    render_montage(rest, os.path.join(PREV_DIR, "hulk_ped_head.png"), views=["front", "three_quarter"],
                   focus=(0, -0.05, 0.68), dist=0.8, samples=64, size=(700, 800))
    export_glb(rest, os.path.join(PREV_DIR, "hulk_ped_rest.glb"))
    for o in rest:
        bpy.data.objects.remove(o)
    # (b) hero pose
    for o in dict_objs.values():
        o.hide_set(False)
    posing.apply_pose(list(dict_objs.values()))
    posed = preview_copies("pose", pmats)
    for o in dict_objs.values():
        o.hide_set(True)
    render_montage(posed, os.path.join(PREV_DIR, "hulk_ped_pose.png"), views=["three_quarter", "front"],
                   focus=(0, 0, -0.1), dist=4.8, samples=48, size=(700, 900))
    export_glb(posed, os.path.join(PREV_DIR, "hulk_ped_pose.glb"))
    log("previews done")

pickle.dump({"parts": {k: (v[1].shape[0]) for k, v in parts.items()}}, open(os.path.join(OUT, ".stats.pkl"), "wb"))
log("done")
