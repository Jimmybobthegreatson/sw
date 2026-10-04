import os, sys, time
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import numpy as np, bpy
from lib.skeleton import Joints
from lib import anatomy
from lib.preview import mesh_from_arrays
J = Joints(); R = anatomy.Rig(J)
cache = "/tmp/claude-0/-home-user-sw/bbe8b9d2-6c61-5aa1-8f13-5b7a5094ca7b/scratchpad/skin3mm.npz"
if os.path.exists(cache):
    d = np.load(cache); v, f = d["v"], d["f"]
else:
    g = anatomy.new_grid(0.003); anatomy.build_skin(g, R); v, f = g.mesh(); np.savez(cache, v=v, f=f)
print("hi", len(v), len(f))
def dist_seg(p, a, b):
    ba = b - a; t = np.clip(((p - a) @ ba) / (ba @ ba), 0, 1)
    return np.linalg.norm(p - (a + t[:, None] * ba), axis=1)
def regions(co):
    ax = np.abs(co[:, 0]); c = co.copy(); c[:, 0] = ax   # mirror to left
    head = c[:, 2] > 0.56
    W = R.wrist; hand = ((c - W) @ ((R.fingers[2][0] - W) / np.linalg.norm(R.fingers[2][0] - W))) > -0.02
    hand &= c[:, 0] > 0.40
    foot = c[:, 2] < -0.90
    return head, hand, foot
head, hand, foot = regions(v)
wd = np.full(len(v), float(sys.argv[5]) if len(sys.argv) > 5 else 1.0, np.float32)
H, Hn, F_ = float(sys.argv[1]), float(sys.argv[2]), float(sys.argv[3])
wd[head] = H; wd[hand] = Hn; wd[foot] = F_
bpy.ops.wm.read_factory_settings(use_empty=True)
me = mesh_from_arrays("hi", v, f)
ob = bpy.data.objects.new("hi", me); bpy.context.collection.objects.link(ob)
vg = ob.vertex_groups.new(name="dens")
for val in np.unique(wd):
    vg.add(np.where(wd == val)[0].tolist(), float(val), "REPLACE")
mod = ob.modifiers.new("dec", "DECIMATE")
mod.decimate_type = "COLLAPSE"; mod.ratio = float(sys.argv[4])
mod.vertex_group = "dens"; mod.vertex_group_factor = 1.0
mod.use_symmetry = True; mod.symmetry_axis = "X"
t = time.time()
ev = ob.evaluated_get(bpy.context.evaluated_depsgraph_get()).to_mesh()
co = np.empty(len(ev.vertices) * 3, np.float32); ev.vertices.foreach_get("co", co); co = co.reshape(-1, 3)
h2, hd2, f2 = regions(co)
print(f"{time.time()-t:.1f}s  tris={len(ev.polygons)} verts={len(co)}  head={h2.sum()} hands(2x)={hd2.sum()} feet={f2.sum()} rest={len(co)-h2.sum()-hd2.sum()-f2.sum()}")
