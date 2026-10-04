"""Mesh processing: decimation (Blender), splitting into ped components, cutting hems, SDF normals.

Everything works on plain numpy arrays ``(verts (N,3) float32, tris (M,3) int32)`` so that the heavy lifting does
not depend on Blender state; ``bpy`` is only used for the decimate modifier and (elsewhere) UV unwrapping.
"""
import numpy as np

from . import anatomy


# --------------------------------------------------------------------------------------------------------------------
# basic mesh helpers
# --------------------------------------------------------------------------------------------------------------------
def face_centroids(v, f):
    return v[f].mean(axis=1)


def face_normals(v, f, normalize=True):
    t = v[f]
    n = np.cross(t[:, 1] - t[:, 0], t[:, 2] - t[:, 0])
    if normalize:
        n /= np.maximum(np.linalg.norm(n, axis=1, keepdims=True), 1e-12)
    return n


def submesh(v, f, keep):
    """Faces selected by boolean mask ``keep``; unused vertices dropped.  Returns (v, f, old_vertex_index)."""
    f = f[keep]
    used, inv = np.unique(f, return_inverse=True)
    return v[used], inv.reshape(-1, 3).astype(np.int32), used


def drop_small_islands(v, f, min_faces=60):
    """Remove connected components with fewer than ``min_faces`` triangles."""
    from scipy.sparse import coo_matrix
    from scipy.sparse.csgraph import connected_components

    n = len(v)
    a = np.concatenate([f[:, 0], f[:, 1], f[:, 2]])
    b = np.concatenate([f[:, 1], f[:, 2], f[:, 0]])
    g = coo_matrix((np.ones(len(a)), (a, b)), shape=(n, n))
    ncomp, lab = connected_components(g, directed=False)
    counts = np.bincount(lab[f[:, 0]], minlength=ncomp)
    keep = counts[lab[f[:, 0]]] >= min_faces
    return submesh(v, f, keep)


def vertex_normals(v, f):
    n = np.zeros_like(v, dtype=np.float64)
    fn = face_normals(v, f, normalize=False)
    # area-weighted
    for k in range(3):
        np.add.at(n, f[:, k], fn)
    ln = np.linalg.norm(n, axis=1, keepdims=True)
    return (n / np.maximum(ln, 1e-12)).astype(np.float32)


# --------------------------------------------------------------------------------------------------------------------
# Blender decimation
# --------------------------------------------------------------------------------------------------------------------
def decimate(v, f, target_tris, weights=None, symmetry=True):
    """Collapse-decimate with Blender down to about ``target_tris`` triangles.  ``weights`` (per vertex, 1 = decimate freely, <1 = protect) steers density."""
    import bpy

    from .preview import mesh_from_arrays

    me = mesh_from_arrays("dec_in", v, f)
    ob = bpy.data.objects.new("dec_in", me)
    bpy.context.collection.objects.link(ob)
    if weights is not None:
        vg = ob.vertex_groups.new(name="dens")
        w = np.round(np.asarray(weights, np.float32), 4)
        for val in np.unique(w):
            vg.add(np.where(w == val)[0].tolist(), float(val), "REPLACE")
    mod = ob.modifiers.new("dec", "DECIMATE")
    mod.decimate_type = "COLLAPSE"
    mod.ratio = float(min(1.0, target_tris / len(f)))
    mod.use_collapse_triangulate = True
    if weights is not None:
        mod.vertex_group = "dens"
        mod.vertex_group_factor = 1.0
    if symmetry:
        mod.use_symmetry = True
        mod.symmetry_axis = "X"
    ev = ob.evaluated_get(bpy.context.evaluated_depsgraph_get()).to_mesh()
    nv = len(ev.vertices)
    co = np.empty(nv * 3, np.float32)
    ev.vertices.foreach_get("co", co)
    ev.calc_loop_triangles()
    idx = np.empty(len(ev.loop_triangles) * 3, np.int32)
    ev.loop_triangles.foreach_get("vertices", idx)
    ob.evaluated_get(bpy.context.evaluated_depsgraph_get()).to_mesh_clear()
    out_v, out_f = co.reshape(-1, 3), idx.reshape(-1, 3)
    bpy.data.objects.remove(ob)
    bpy.data.meshes.remove(me)
    return out_v, out_f


# --------------------------------------------------------------------------------------------------------------------
# part classification (face centroids -> part id)
# --------------------------------------------------------------------------------------------------------------------
class Parts:
    UPPR, HEAD, HAND, FEET, DROP = range(5)
    NAMES = {UPPR: "uppr", HEAD: "head", HAND: "hand", FEET: "feet", DROP: "drop"}


def classify_skin(c, R):
    """Part id of every skin face given its centroid ``c`` (N,3)."""
    m = c.copy()
    ax = np.abs(m[:, 0])
    m[:, 0] = ax  # work on the left side
    label = np.full(len(c), Parts.UPPR, np.int32)

    # lower body: legs below the hips are covered by the jeans; only the shins + feet are rendered
    knee_z = R.knee[2] + 0.02
    label[m[:, 2] < anatomy.SKIN_UPPR_BOTTOM] = Parts.DROP
    label[m[:, 2] < knee_z] = Parts.FEET

    # hands: beyond the wrist plane (perpendicular to the forearm)
    fore = (R.wrist - R.elbow) / np.linalg.norm(R.wrist - R.elbow)
    beyond = (m - R.wrist) @ fore
    label[(beyond > 0.0) & (m[:, 0] > 0.38) & (m[:, 2] > -0.35)] = Parts.HAND

    # head + neck
    label[(m[:, 2] > anatomy.NECK_CUT_Z) & (ax < anatomy.NECK_CUT_X)] = Parts.HEAD
    return label


def density_weights_skin(v, R, head_weight=0.995):
    """Per-vertex 'decimation amount' for the skin mesh (1 = full, slightly lower = keep more detail)."""
    w = np.ones(len(v), np.float32)
    w[(v[:, 2] > anatomy.NECK_CUT_Z - 0.01) & (np.abs(v[:, 0]) < anatomy.NECK_CUT_X + 0.01)] = head_weight
    return w


def decimate_skin(grid, v, f, R, total=19500, head_target=5200, tries=(0.995, 0.99, 0.98, 0.96, 0.93)):
    """Decimate the skin sculpt so the head keeps about ``head_target`` triangles (the weight needed depends on the
    sculpt resolution, so it is searched for)."""
    best = None
    if len(f) > 650000:   # the density weights behave well on ~0.5M-triangle input: reduce uniformly first
        v, f = decimate(v, f, 600000, None)
        v = project_to_surface(grid, v)
    for hw in tries:
        dv, df = decimate(v, f, total, density_weights_skin(v, R, hw))
        dv, df = cleanup(grid, dv, df)
        lab = classify_skin(face_centroids(dv, df), R)
        n_head = int((lab == Parts.HEAD).sum())
        if best is None or abs(n_head - head_target) < abs(best[2] - head_target):
            best = (dv, df, n_head)
        if n_head >= head_target * 0.8:
            break
    return best[0], best[1]


def hem_height(x, y, side):
    """Torn hem height for the jeans: a diagonal tear with ragged teeth."""
    base = anatomy.HEM_Z[0] if side > 0 else anatomy.HEM_Z[1]
    cx = (0.127 if side > 0 else -0.127)
    th = np.arctan2(y - 0.03, (x - cx) * side)
    tri = np.abs(((th / (2 * np.pi) * 7.0 + 0.13) % 1.0) - 0.5) * 2.0  # triangle wave, 7 teeth around the leg
    return base + 0.050 * np.sin(th + (0.6 if side > 0 else 2.4)) + 0.020 * (tri - 0.5) + 0.008 * np.sin(13 * th + 2.0)


def cut_jeans(v, f):
    c = face_centroids(v, f)
    waist = anatomy.WAIST_Z + anatomy.WAIST_TILT * c[:, 1]
    keep = c[:, 2] < waist
    side = np.where(c[:, 0] >= 0, 1.0, -1.0)
    hem = np.where(side > 0, hem_height(c[:, 0], c[:, 1], 1), hem_height(c[:, 0], c[:, 1], -1))
    keep &= c[:, 2] > hem
    # nothing of the jeans should exist below the hem band
    return submesh(v, f, keep)


def density_weights_jeans(v):
    w = np.ones(len(v), np.float32)
    band = (v[:, 2] < anatomy.HEM_Z[1] + 0.07) & (v[:, 2] > anatomy.HEM_Z[0] - 0.09)
    w[band] = 0.985
    top = (v[:, 2] > anatomy.WAIST_Z - 0.04)
    w[top] = 0.99
    return w


def cut_hair(v, f):
    c = face_centroids(v, f)
    keep = c[:, 2] > anatomy.hairline_z(c[:, 0], c[:, 1])
    return drop_small_islands(*submesh(v, f, keep)[:2], min_faces=30)[:2]


# --------------------------------------------------------------------------------------------------------------------
# normals from the SDF
# --------------------------------------------------------------------------------------------------------------------
def sdf_normals(grid, v):
    """Smooth normals taken from the signed distance field of the full-resolution sculpt (outward)."""
    return grid.gradient(v, eps=grid.h * 1.5).astype(np.float32)


def fix_winding(v, f, grid, eps=0.01):
    """Flip triangles whose geometric normal points into the body (fold-overs left behind by decimation)."""
    c = face_centroids(v, f)
    sn = grid.gradient(c, eps=eps)
    fn = face_normals(v, f)
    bad = (fn * sn).sum(1) < 0.0
    f = f.copy()
    f[bad] = f[bad][:, [0, 2, 1]]
    return f, int(bad.sum())


def project_to_surface(grid, v, iters=2, max_step=0.02):
    """Newton-project vertices onto the zero level set of the SDF (restores volume lost by decimation)."""
    p = v.astype(np.float64)
    for _ in range(iters):
        d = grid.sample(p)
        g = grid.gradient(p, eps=max(grid.h, 0.003))
        p = p - np.clip(d, -max_step, max_step)[:, None] * g
    return p.astype(np.float32)


def cleanup(grid, v, f):
    v = project_to_surface(grid, v)
    f, n = fix_winding(v, f, grid)
    return v, f


def weld(v, f, decimals=6):
    """Merge vertices that share a position (undo UV-seam splitting)."""
    key = np.round(v.astype(np.float64), decimals)
    _, first, inv = np.unique(key, axis=0, return_index=True, return_inverse=True)
    wf = inv[f].astype(np.int32)
    keep = (wf[:, 0] != wf[:, 1]) & (wf[:, 1] != wf[:, 2]) & (wf[:, 0] != wf[:, 2])
    return v[first], wf[keep]


def _closest_bary(p, a, b, c):
    """Barycentric coordinates (w_a, w_b, w_c) of the closest point on triangle abc to p (vectorised, Ericson)."""
    ab, ac, ap = b - a, c - a, p - a
    d1, d2 = (ab * ap).sum(-1), (ac * ap).sum(-1)
    bp = p - b
    d3, d4 = (ab * bp).sum(-1), (ac * bp).sum(-1)
    cp = p - c
    d5, d6 = (ab * cp).sum(-1), (ac * cp).sum(-1)
    vc = d1 * d4 - d3 * d2
    vb = d5 * d2 - d1 * d6
    va = d3 * d6 - d5 * d4
    out = np.zeros(p.shape[:-1] + (3,))
    denom = va + vb + vc
    denom = np.where(np.abs(denom) < 1e-18, 1e-18, denom)
    v_ = vb / denom
    w_ = vc / denom
    out[..., 0], out[..., 1], out[..., 2] = 1 - v_ - w_, v_, w_
    # clamp to the triangle by projecting onto the nearest feature when outside
    outside = (out < 0).any(-1)
    if outside.any():
        idx = np.where(outside)
        best = None
        for (i, j) in ((0, 1), (1, 2), (2, 0)):
            P = [a, b, c]
            e0, e1 = P[i][idx], P[j][idx]
            d = e1 - e0
            t = np.clip(((p[idx] - e0) * d).sum(-1) / np.maximum((d * d).sum(-1), 1e-18), 0, 1)
            q = e0 + d * t[..., None]
            dist = ((p[idx] - q) ** 2).sum(-1)
            bw = np.zeros(dist.shape + (3,))
            bw[..., i] = 1 - t
            bw[..., j] = t
            if best is None:
                best = (dist, bw)
            else:
                m = dist < best[0]
                best = (np.where(m, dist, best[0]), np.where(m[..., None], bw, best[1]))
        out[idx] = best[1]
    return out


def lod_uvs(hd_v, hd_f, hd_uv, lv, lf, k=6):
    """UVs for a decimated copy of the (split-vertex) high LOD.  Returns (verts, tris, uv) re-split along UV islands."""
    from scipy.sparse import coo_matrix
    from scipy.sparse.csgraph import connected_components
    from scipy.spatial import cKDTree

    nh = len(hd_v)
    e = np.concatenate([hd_f[:, [0, 1]], hd_f[:, [1, 2]]])
    _, comp_v = connected_components(coo_matrix((np.ones(len(e)), (e[:, 0], e[:, 1])), shape=(nh, nh)), directed=False)
    comp_t = comp_v[hd_f[:, 0]]
    hd_cent = hd_v[hd_f].mean(1)
    tree = cKDTree(hd_cent)
    lcent = lv[lf].mean(1)
    _, nn = tree.query(lcent)
    face_isl = comp_t[nn]
    corner_uv = np.zeros((len(lf), 3, 2))
    for isl in np.unique(face_isl):
        tris = np.where(comp_t == isl)[0]
        sub = cKDTree(hd_cent[tris])
        fsel = np.where(face_isl == isl)[0]
        P = lv[lf[fsel]].astype(np.float64)               # (m, 3, 3)
        kk = min(k, len(tris))
        _, cand = sub.query(P.reshape(-1, 3), k=kk)
        cand = tris[cand.reshape(-1, kk)]                   # (m*3, kk) indices into hd tris
        pts = np.repeat(P.reshape(-1, 3)[:, None, :], kk, axis=1)
        A, B, C = hd_v[hd_f[cand, 0]], hd_v[hd_f[cand, 1]], hd_v[hd_f[cand, 2]]
        bw = _closest_bary(pts, A.astype(np.float64), B.astype(np.float64), C.astype(np.float64))
        q = bw[..., 0:1] * A + bw[..., 1:2] * B + bw[..., 2:3] * C
        d2 = ((q - pts) ** 2).sum(-1)
        best = d2.argmin(1)
        ii = np.arange(len(best))
        tb = cand[ii, best]
        w = bw[ii, best]
        uvq = w[:, 0:1] * hd_uv[hd_f[tb, 0]] + w[:, 1:2] * hd_uv[hd_f[tb, 1]] + w[:, 2:3] * hd_uv[hd_f[tb, 2]]
        corner_uv[fsel] = uvq.reshape(-1, 3, 2)
    key = lf.astype(np.int64) * (int(face_isl.max()) + 1) + face_isl[:, None]
    uniq, first_idx, inv = np.unique(key.ravel(), return_index=True, return_inverse=True)
    verts = lv[lf.ravel()[first_idx]]
    uvs = corner_uv.reshape(-1, 2)[first_idx]
    return verts, inv.reshape(-1, 3).astype(np.int32), uvs.astype(np.float32)


def decimate_uv(v, f, uv, target_tris):
    """LOD copy of a split-vertex mesh: decimate the welded surface, then re-derive UVs from the original islands."""
    wv, wf = weld(v, f)
    lv, lf = decimate(wv, wf, target_tris, None, symmetry=False)
    return lod_uvs(v, f, uv, lv, lf)
