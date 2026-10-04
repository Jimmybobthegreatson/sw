import os, sys, pickle, time
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import numpy as np, xatlas
from scipy.sparse import coo_matrix, diags
from PIL import Image, ImageDraw
S = "/tmp/claude-0/-home-user-sw/bbe8b9d2-6c61-5aa1-8f13-5b7a5094ca7b/scratchpad/"
res = pickle.load(open(S + "parts_cache.pkl", "rb"))

def laplacian(v, f):
    n = len(v)
    e = np.concatenate([f[:, [0, 1]], f[:, [1, 2]], f[:, [2, 0]]])
    A = coo_matrix((np.ones(len(e)), (e[:, 0], e[:, 1])), shape=(n, n)).tocsr()
    A = ((A + A.T) > 0).astype(np.float64)
    d = np.asarray(A.sum(1)).ravel()
    return diags(1 / np.maximum(d, 1)) @ A

def taubin(v, f, iters=40, lam=0.55, mu=-0.58):
    L = laplacian(v, f); p = v.astype(np.float64).copy()
    for i in range(iters):
        p += lam * (L @ p - p)
        p += mu * (L @ p - p)
    return p

for name in ("head", "uppr", "hand", "feet", "jeans", "hair"):
    v, f = res[name]
    for it in (0, 25, 60, 150):
        p = taubin(v, f, it) if it else v.astype(np.float64)
        atlas = xatlas.Atlas(); atlas.add_mesh(p.astype(np.float32), f.astype(np.uint32))
        co = xatlas.ChartOptions(); co.max_iterations = 2
        po = xatlas.PackOptions(); po.resolution = 1024; po.padding = 5; po.rotate_charts = True; po.bilinear = True
        t = time.time(); atlas.generate(co, po, verbose=False)
        vmap, idx, uv = atlas[0]
        print(f"{name:6s} smooth_iters={it:3d} charts={atlas.chart_count:4d} util={atlas.utilization*100:.0f}% verts {len(v)}->{len(vmap)} {time.time()-t:.1f}s")
