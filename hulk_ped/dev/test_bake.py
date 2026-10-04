import os, sys, pickle, time
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import numpy as np, bpy
from lib.sdf import Grid
from lib import uvmap, bake, shading, meshing
from lib.preview import mesh_with_uv, flat_mat, render_montage, mesh_from_arrays
S = "/tmp/claude-0/-home-user-sw/bbe8b9d2-6c61-5aa1-8f13-5b7a5094ca7b/scratchpad/"
res = pickle.load(open(S + "parts_cache.pkl", "rb"))
grids = {k: Grid.from_array(*res[f"_{k}_grid_F"]) for k in ("skin", "jeans", "hair")}
cfg = {"head": ("skin", 1024), "uppr": ("skin", 1024), "hand": ("skin", 512), "feet": ("skin", 512), "jeans": ("jeans", 1024), "hair": ("hair", 256)}
N = bake.Noise3(5)
bpy.ops.wm.read_factory_settings(use_empty=True)
out = {}
for name, (gk, R) in cfg.items():
    t = time.time()
    g = grids[gk]
    v, f = res[name]
    vmap, nf, uv, st = uvmap.unwrap(v, f, lambda p: g.gradient(p, eps=0.014), resolution=R, padding=4)
    nv = v[vmap]
    vn = g.gradient(nv, eps=0.010)
    pm = bake.PartMaps(nv, nf, uv, vn, R)
    pm.project(g)
    ao = bake.ambient_occlusion(g, pm.spos, pm.snrm)
    if gk == "skin":
        col, spec = shading.skin(pm.spos, pm.snrm, ao, N, name)
    elif gk == "jeans":
        col, spec = shading.denim(pm.spos, pm.snrm, ao, N)
    else:
        col, spec = shading.hair(pm.spos, pm.snrm, ao, N)
    dx, dy = bake.micro_detail(R)
    det = (dx[pm.mask], dy[pm.mask])
    nmap = bake.normal_map(pm, g, detail=det, detail_strength=0.10 if gk != "hair" else 0.0)
    diff = pm.to_image(col)
    out[name] = (nv, nf, uv, vn, diff, nmap)
    print(name, st["islands"], f"{time.time()-t:.1f}s")
# build textured preview
for name, (nv, nf, uv, vn, diff, nmap) in out.items():
    R = diff.shape[0]
    from PIL import Image as _I
    _I.fromarray((np.clip(diff, 0, 1) * 255).astype(np.uint8)).save(S + f"bk_{name}_d.png")
    _I.fromarray((np.clip(nmap, 0, 1) * 255).astype(np.uint8)).save(S + f"bk_{name}_n.png")
    di = bpy.data.images.load(S + f"bk_{name}_d.png"); ni = bpy.data.images.load(S + f"bk_{name}_n.png"); ni.colorspace_settings.name = "Non-Color"
    me = mesh_with_uv(name, nv, nf, uv)
    for p_ in me.polygons: p_.use_smooth = True
    me.normals_split_custom_set_from_vertices([tuple(x) for x in vn])
    ob = bpy.data.objects.new(name, me); bpy.context.collection.objects.link(ob)
    m = bpy.data.materials.new(name)
    if bpy.app.version < (5, 0, 0): m.use_nodes = True
    nt = m.node_tree; bsdf = nt.nodes["Principled BSDF"]
    t1 = nt.nodes.new("ShaderNodeTexImage"); t1.image = di
    t2 = nt.nodes.new("ShaderNodeTexImage"); t2.image = ni
    nm = nt.nodes.new("ShaderNodeNormalMap")
    nt.links.new(t1.outputs["Color"], bsdf.inputs["Base Color"]); nt.links.new(t2.outputs["Color"], nm.inputs["Color"]); nt.links.new(nm.outputs["Normal"], bsdf.inputs["Normal"])
    bsdf.inputs["Roughness"].default_value = 0.5 if name != "jeans" else 0.85
    me.materials.append(m)
ground = bpy.data.objects.new("g", mesh_from_arrays("g", np.array([[-3,-3,-1.0],[3,-3,-1.0],[3,3,-1.0],[-3,3,-1.0]],np.float32), np.array([[0,1,2],[0,2,3]],np.int32)))
bpy.context.collection.objects.link(ground); ground.data.materials.append(flat_mat("floor", (0.25,0.25,0.27), 0.9))
render_montage([], os.path.join(os.path.dirname(os.path.abspath(__file__)), "baked.png"), views=["front", "three_quarter", "back"], focus=(0, 0, -0.1), dist=4.6, samples=24, size=(520, 780))
render_montage([], os.path.join(os.path.dirname(os.path.abspath(__file__)), "baked_head.png"), views=["front", "three_quarter"], focus=(0, -0.05, 0.68), dist=0.7, samples=24, size=(600, 680))
from PIL import Image
for name,(nv,nf,uv,vn,diff,nmap) in out.items():
    print(name, "diff mean", diff.mean(), diff.min(), diff.max(), "nmap mean", nmap.mean(0).mean(0))
    Image.fromarray((np.clip(diff,0,1)*255).astype(np.uint8)).save(S+f"bk_{name}_d.png")
    Image.fromarray((np.clip(nmap,0,1)*255).astype(np.uint8)).save(S+f"bk_{name}_n.png")
pickle.dump(out, open(S + "baked_out.pkl", "wb"))
