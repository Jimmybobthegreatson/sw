"""Dev: build all parts once (coarse voxel size) and cache them as npz for fast UV/weight/texture experiments."""
import os, sys, time, pickle
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import numpy as np, bpy
from lib.skeleton import Joints
from lib import anatomy, meshing
from lib.meshing import Parts
H = float(sys.argv[1]) if len(sys.argv) > 1 else 0.004
OUT = "/tmp/claude-0/-home-user-sw/bbe8b9d2-6c61-5aa1-8f13-5b7a5094ca7b/scratchpad/parts_cache.pkl"
J = Joints(); R = anatomy.Rig(J)
bpy.ops.wm.read_factory_settings(use_empty=True)
res = {}
g = anatomy.new_grid(H); anatomy.build_skin(g, R); v, f = g.mesh()
dv, df = meshing.decimate(v, f, 19000, meshing.density_weights_skin(v, R))
dv, df = meshing.cleanup(g, dv, df)
lab = meshing.classify_skin(meshing.face_centroids(dv, df), R)
for pid in (Parts.UPPR, Parts.HEAD, Parts.HAND, Parts.FEET):
    sv, sf, _ = meshing.submesh(dv, df, lab == pid)
    res[Parts.NAMES[pid]] = (sv, sf)
res["_skin_grid_F"] = (g.F, g.lo, g.h)
gj = anatomy.new_grid(H); anatomy.build_jeans_full(gj, R); jv, jf = gj.mesh()
jd, jdf = meshing.decimate(jv, jf, 6500, meshing.density_weights_jeans(jv))
jd, jdf = meshing.cleanup(gj, jd, jdf)
res["jeans"] = meshing.cut_jeans(jd, jdf)[:2]
res["_jeans_grid_F"] = (gj.F, gj.lo, gj.h)
gh = anatomy.new_grid(H); anatomy.build_hair_full(gh, R); hv, hf = gh.mesh()
hd, hdf = meshing.decimate(hv, hf, 2200)
hd, hdf = meshing.cleanup(gh, hd, hdf)
res["hair"] = meshing.cut_hair(hd, hdf)
res["_hair_grid_F"] = (gh.F, gh.lo, gh.h)
pickle.dump(res, open(OUT, "wb"))
print({k: len(v[1]) for k, v in res.items() if not k.startswith("_")})
