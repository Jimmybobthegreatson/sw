import os, sys, pickle, time
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import numpy as np, xatlas, bpy
exec(open("test_uv3.py").read().split("for name in")[0])
from lib.preview import mesh_with_uv, checker_image, textured_mat, flat_mat, render_montage, mesh_from_arrays
from lib.sdf import Grid
from lib import uvmap
bpy.ops.wm.read_factory_settings(use_empty=True)
img = checker_image()
mat = textured_mat("chk", img)
v, f = res["uppr"]
grid = Grid.from_array(*res["_skin_grid_F"])
# candidate A: xatlas on Taubin-smoothed mesh
p = taubin(v, f, 60)
atlas = xatlas.Atlas(); atlas.add_mesh(p.astype(np.float32), f.astype(np.uint32))
co = xatlas.ChartOptions(); co.max_iterations = 3
po = xatlas.PackOptions(); po.resolution = 1024; po.padding = 5; po.rotate_charts = True
atlas.generate(co, po, verbose=False)
vmapA, idxA, uvA = atlas[0]
pass
# candidate B: structured planar islands (with the flip repair)
vmapB, idxB, uvB, st = uvmap.unwrap(v, f, lambda q: grid.gradient(q, eps=0.014), resolution=1024)
print("B", st)
for k, (vm, ix, uv) in {"A": (vmapA, idxA, uvA), "B": (vmapB, idxB, uvB)}.items():
    me = mesh_with_uv("m" + k, v[vm], ix, uv)
    ob = bpy.data.objects.new("m" + k, me); bpy.context.collection.objects.link(ob)
    me.materials.append(mat)
    ob.location = (0.0 if k == "A" else 1.9, 0, 0)
render_montage([], os.path.join(os.path.dirname(os.path.abspath(__file__)), "uv_compare.png"), views=["front", "side", "back"], focus=(0.95, 0, 0.2), dist=4.0, samples=12, size=(700, 520))
