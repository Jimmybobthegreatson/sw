import os, sys, pickle, time
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import numpy as np, xatlas
from PIL import Image, ImageDraw
res = pickle.load(open("/tmp/claude-0/-home-user-sw/bbe8b9d2-6c61-5aa1-8f13-5b7a5094ca7b/scratchpad/parts_cache.pkl", "rb"))
for name in ("head", "uppr", "hand", "feet", "jeans", "hair"):
    v, f = res[name]
    t = time.time()
    atlas = xatlas.Atlas()
    atlas.add_mesh(v, f)
    co = xatlas.ChartOptions(); co.max_iterations = 2
    po = xatlas.PackOptions(); po.resolution = 1024; po.padding = 6; po.bilinear = True; po.rotate_charts = True; po.texels_per_unit = 0
    atlas.generate(co, po, verbose=False)
    vmap, idx, uv = atlas[0]
    # chart count and utilisation estimate
    img = Image.new("L", (1024, 1024), 0); d = ImageDraw.Draw(img)
    area = 0
    for tri in idx:
        p = [(uv[i][0] * 1023, (1 - uv[i][1]) * 1023) for i in tri]
        d.polygon(p, fill=255, outline=120)
    util = (np.array(img) > 0).mean()
    print(f"{name:6s} verts {len(v)} -> {len(vmap)}  tris {len(f)}  utilisation {util*100:.0f}%  {time.time()-t:.1f}s  atlas {atlas.width}x{atlas.height} charts {atlas.chart_count}")
    img.save(f"/tmp/claude-0/-home-user-sw/bbe8b9d2-6c61-5aa1-8f13-5b7a5094ca7b/scratchpad/uv_{name}.png")
