import os, sys, pickle, time
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import numpy as np
from PIL import Image, ImageDraw
from lib.sdf import Grid
from lib import uvmap
S = "/tmp/claude-0/-home-user-sw/bbe8b9d2-6c61-5aa1-8f13-5b7a5094ca7b/scratchpad/"
res = pickle.load(open(S + "parts_cache.pkl", "rb"))
grids = {k: Grid.from_array(*res[f"_{k}_grid_F"]) for k in ("skin", "jeans", "hair")}
gmap = {"head": "skin", "uppr": "skin", "hand": "skin", "feet": "skin", "jeans": "jeans", "hair": "hair"}
for name in ("head", "uppr", "hand", "feet", "jeans", "hair"):
    v, f = res[name]
    g = grids[gmap[name]]
    t = time.time()
    vmap, nf, uv, st = uvmap.unwrap(v, f, lambda p: g.gradient(p, eps=0.014), resolution=1024, padding=5)
    img = Image.new("RGB", (1024, 1024), (0, 0, 0)); d = ImageDraw.Draw(img)
    rng = np.random.default_rng(1)
    for tri in nf:
        p = [(uv[i][0] * 1023, uv[i][1] * 1023) for i in tri]
        d.polygon(p, fill=(255, 255, 255), outline=(90, 90, 90))
    cover = (np.array(img)[:, :, 0] > 0).mean()
    print(f"{name:6s} tris {len(f)} verts {len(v)}->{len(vmap)} islands={st['islands']} tpm={st['texels_per_meter']:.0f} util={st['utilization']*100:.0f}% charts={st['charts']} flipped={st['flipped']} atlas={st['size']} coverage={cover*100:.0f}% {time.time()-t:.1f}s")
    img.save(S + f"uv2_{name}.png")
