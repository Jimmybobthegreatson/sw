import os, sys, pickle, time
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import numpy as np, xatlas
exec(open("test_uv3.py").read().split("for name in")[0])   # reuse laplacian/taubin/res

def stretch(p3, uv, idx):
    """area weighted mean anisotropy (sigma_max/sigma_min) and 95th percentile; uv in arbitrary units -> normalised by area ratio"""
    t = p3[idx]; q = uv[idx]
    e1, e2 = t[:, 1] - t[:, 0], t[:, 2] - t[:, 0]
    a1, a2 = q[:, 1] - q[:, 0], q[:, 2] - q[:, 0]
    A3 = np.linalg.norm(np.cross(e1, e2), axis=1) + 1e-18
    A2 = np.abs(a1[:, 0] * a2[:, 1] - a1[:, 1] * a2[:, 0]) + 1e-18
    # Sander metric via first fundamental form of the map uv->3D
    # Ss = (e1*a2.y - e2*a1.y)/det ; St = (-e1*a2.x + e2*a1.x)/det
    det = (a1[:, 0] * a2[:, 1] - a1[:, 1] * a2[:, 0])
    det = np.where(np.abs(det) < 1e-18, 1e-18, det)
    Ss = (e1 * a2[:, 1:2] - e2 * a1[:, 1:2]) / det[:, None]
    St = (-e1 * a2[:, 0:1] + e2 * a1[:, 0:1]) / det[:, None]
    a = (Ss * Ss).sum(1); b = (Ss * St).sum(1); c = (St * St).sum(1)
    disc = np.sqrt(np.maximum((a - c) ** 2 + 4 * b * b, 0))
    smax = np.sqrt(0.5 * (a + c + disc)); smin = np.sqrt(np.maximum(0.5 * (a + c - disc), 1e-18))
    ratio = smax / smin
    w = A3 / A3.sum()
    return float((ratio * w).sum()), float(np.percentile(ratio, 95))

for name in ("uppr", "feet", "jeans"):
    v, f = res[name]
    p = taubin(v, f, 60)
    for mc, nd, ns, ts in ((2, 2, 4, .5), (4, 1.0, 4, .5), (8, 1.0, 2, .5), (16, 0.5, 2, .5), (40, 0.3, 1, .5), (16, 0.5, 8, 4)):
        atlas = xatlas.Atlas(); atlas.add_mesh(p.astype(np.float32), f.astype(np.uint32))
        co = xatlas.ChartOptions(); co.max_iterations = 3; co.max_cost = mc; co.normal_deviation_weight = nd; co.normal_seam_weight = ns; co.texture_seam_weight = ts
        po = xatlas.PackOptions(); po.resolution = 1024; po.padding = 5; po.rotate_charts = True; po.bilinear = True
        atlas.generate(co, po, verbose=False)
        vmap, idx, uv = atlas[0]
        m, p95 = stretch(v[vmap].astype(np.float64), uv.astype(np.float64), idx.astype(np.int64))
        print(f"{name:6s} maxcost={mc:3d} ndw={nd} nsw={ns} tsw={ts}: charts={atlas.chart_count:4d} util={atlas.utilization*100:.0f}% verts {len(v)}->{len(vmap)} stretch mean={m:.2f} p95={p95:.2f}")
