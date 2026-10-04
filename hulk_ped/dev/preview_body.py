"""Dev tool: build the skin SDF at a given voxel size and render a quick turnaround.

usage: preview_body.py [--h 0.008] [--out file.png] [--views front,side] [--focus 0,0,-0.1] [--dist 4.6] [--lens 70]
"""
import argparse, os, sys, time
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
import numpy as np
import bpy
from lib.skeleton import Joints
from lib import anatomy
from lib.preview import mesh_from_arrays, flat_mat, render_montage

ap = argparse.ArgumentParser()
ap.add_argument("--h", type=float, default=0.008)
ap.add_argument("--out", default=os.path.join(HERE, "preview_body.png"))
ap.add_argument("--views", default="front,three_quarter,side,back")
ap.add_argument("--focus", default="0,0,-0.1")
ap.add_argument("--dist", type=float, default=4.6)
ap.add_argument("--size", default="480,760")
ap.add_argument("--samples", type=int, default=16)
ap.add_argument("--part", default="skin", help="skin|jeans|hair|all")
a = ap.parse_args()
t = time.time()
J = Joints(); R = anatomy.Rig(J)
g = anatomy.new_grid(a.h)
{"skin": anatomy.build_skin, "jeans": anatomy.build_jeans_full, "hair": anatomy.build_hair_full}[a.part](g, R)
v, f = g.mesh()
print(f"sdf+mesh {time.time()-t:.1f}s  verts={len(v)} faces={len(f)}")
bpy.ops.wm.read_factory_settings(use_empty=True)
me = mesh_from_arrays("body", v, f)
ob = bpy.data.objects.new("body", me); bpy.context.collection.objects.link(ob)
for p in me.polygons: p.use_smooth = True
me.materials.append(flat_mat("skin", {"skin": (0.18, 0.40, 0.06), "jeans": (0.12, 0.2, 0.4), "hair": (0.03, 0.03, 0.03)}[a.part]))
ground = bpy.data.objects.new("g", mesh_from_arrays("g", np.array([[-3,-3,-1.0],[3,-3,-1.0],[3,3,-1.0],[-3,3,-1.0]],np.float32), np.array([[0,1,2],[0,2,3]],np.int32)))
bpy.context.collection.objects.link(ground)
ground.data.materials.append(flat_mat("floor", (0.25,0.25,0.27), 0.9))
# placeholder eyeballs so the sockets can be judged
import bmesh
if a.part != "skin": a.__dict__["skip_eyes"] = True
eyemat = flat_mat("eye", (0.9, 0.9, 0.85), 0.2); irismat = flat_mat("iris", (0.05, 0.12, 0.04), 0.2)
for sx in (1, -1) if a.part == "skin" else ():
    for rad, off, mat in ((0.0135, 0.0, eyemat), (0.0074, -0.0079, irismat)):
        bm = bmesh.new(); bmesh.ops.create_uvsphere(bm, u_segments=24, v_segments=16, radius=rad)
        m = bpy.data.meshes.new("eye"); bm.to_mesh(m); bm.free()
        for pl in m.polygons: pl.use_smooth = True
        o = bpy.data.objects.new("eye", m); bpy.context.collection.objects.link(o)
        e = R.eye; o.location = (sx * e[0], e[1] + off, e[2]); m.materials.append(mat)
t = time.time()
focus = tuple(float(x) for x in a.focus.split(","))
size = tuple(int(x) for x in a.size.split(","))
render_montage([ob], a.out, views=a.views.split(","), focus=focus, dist=a.dist, samples=a.samples, size=size)
print(f"render {time.time()-t:.1f}s -> {a.out}")
