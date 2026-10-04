import os, sys, time
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import numpy as np, bpy
from lib.skeleton import Joints
from lib import anatomy
from lib.preview import mesh_from_arrays

J = Joints(); R = anatomy.Rig(J)
t = time.time()
g = anatomy.new_grid(0.003); anatomy.build_skin(g, R); v, f = g.mesh()
print("hi", len(v), len(f), f"{time.time()-t:.1f}s")
bpy.ops.wm.read_factory_settings(use_empty=True)
me = mesh_from_arrays("hi", v, f)
ob = bpy.data.objects.new("hi", me); bpy.context.collection.objects.link(ob)
# density: 1 in head/hands/feet, 0.1 elsewhere
head = np.linalg.norm((v - np.array([0, -0.03, 0.67])) * np.array([1, 1, 1]), axis=1) < 0.14
w = np.where(head, 1.0, 0.1).astype(np.float32)
vg = ob.vertex_groups.new(name="dens")
for val in (1.0, 0.1):
    idx = np.where(np.isclose(w, val))[0].tolist()
    vg.add(idx, val, "REPLACE")
mod = ob.modifiers.new("dec", "DECIMATE")
mod.decimate_type = "COLLAPSE"
mod.ratio = 0.012
mod.vertex_group = "dens"
mod.vertex_group_factor = float(sys.argv[1]) if len(sys.argv) > 1 else 1.0
mod.invert_vertex_group = (len(sys.argv) > 2 and sys.argv[2] == "inv")
mod.use_symmetry = True; mod.symmetry_axis = "X"
t = time.time()
dg = bpy.context.evaluated_depsgraph_get()
ev = ob.evaluated_get(dg).to_mesh()
n = len(ev.polygons)
co = np.empty(len(ev.vertices) * 3, np.float32); ev.vertices.foreach_get("co", co); co = co.reshape(-1, 3)
print(f"decimate {time.time()-t:.1f}s -> {n} tris")
hd = (np.linalg.norm(co - np.array([0, -0.03, 0.67]), axis=1) < 0.14).sum()
print("verts in head sphere:", hd, "of", len(co))
