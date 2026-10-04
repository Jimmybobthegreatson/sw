import os, sys, time
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import numpy as np, bpy
from lib.skeleton import Joints
from lib import anatomy, meshing
from lib.meshing import Parts
from lib.preview import mesh_from_arrays, flat_mat, render_montage
J = Joints(); R = anatomy.Rig(J)
H = float(sys.argv[1]) if len(sys.argv) > 1 else 0.004
t = time.time()
parts = {}
g = anatomy.new_grid(H); anatomy.build_skin(g, R); v, f = g.mesh(); print("skin hi", len(f), f"{time.time()-t:.0f}s")
dv, df = meshing.decimate(v, f, 19000, meshing.density_weights_skin(v, R))
print("skin lo", len(dv), len(df))
lab = meshing.classify_skin(meshing.face_centroids(dv, df), R)
cols = {Parts.UPPR: (0.15, 0.45, 0.08), Parts.HEAD: (0.6, 0.55, 0.1), Parts.HAND: (0.7, 0.2, 0.1), Parts.FEET: (0.1, 0.4, 0.6)}
objs = []
bpy.ops.wm.read_factory_settings(use_empty=True)
for pid, col in cols.items():
    sv, sf, _ = meshing.submesh(dv, df, lab == pid)
    print(Parts.NAMES[pid], len(sf), "tris")
    me = mesh_from_arrays(Parts.NAMES[pid], sv, sf); ob = bpy.data.objects.new(Parts.NAMES[pid], me); bpy.context.collection.objects.link(ob)
    me.materials.append(flat_mat(Parts.NAMES[pid], col))
# jeans
t = time.time()
gj = anatomy.new_grid(H); anatomy.build_jeans_full(gj, R); jv, jf = gj.mesh()
jd, jdf = meshing.decimate(jv, jf, 6500, meshing.density_weights_jeans(jv))
jc, jcf, _ = meshing.cut_jeans(jd, jdf); print("jeans", len(jcf))
me = mesh_from_arrays("jeans", jc, jcf); ob = bpy.data.objects.new("jeans", me); bpy.context.collection.objects.link(ob); me.materials.append(flat_mat("j", (0.1, 0.18, 0.4)))
# hair
gh = anatomy.new_grid(H); anatomy.build_hair_full(gh, R); hv, hf = gh.mesh()
hd, hdf = meshing.decimate(hv, hf, 2200)
hc, hcf = meshing.cut_hair(hd, hdf); print("hair", len(hcf))
me = mesh_from_arrays("hair", hc, hcf); ob = bpy.data.objects.new("hair", me); bpy.context.collection.objects.link(ob); me.materials.append(flat_mat("h", (0.03, 0.03, 0.03)))
for ob in bpy.data.objects:
    for p in ob.data.polygons: p.use_smooth = False
ground = bpy.data.objects.new("g", mesh_from_arrays("g", np.array([[-3,-3,-1.0],[3,-3,-1.0],[3,3,-1.0],[-3,3,-1.0]],np.float32), np.array([[0,1,2],[0,2,3]],np.int32)))
bpy.context.collection.objects.link(ground); ground.data.materials.append(flat_mat("floor", (0.25,0.25,0.27), 0.9))
render_montage([], "/home/user/sw/hulk_ped/dev/parts.png", views=["front", "three_quarter", "back"], focus=(0, 0, -0.1), dist=4.6, samples=16, size=(520, 780))
