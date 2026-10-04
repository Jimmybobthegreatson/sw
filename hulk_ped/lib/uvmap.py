"""Structured UV unwrapping for the ped parts.

Faces are labelled by their dominant (smoothed) normal direction, labels are cleaned up so that islands stay large,
every island is projected along its axis, and the islands are packed with xatlas.  Compared to xatlas' own chart
generator this produces a handful of fat, low-distortion islands instead of hundreds of shards on a creased mesh.
"""
import numpy as np
from scipy.sparse import coo_matrix
from scipy.sparse.csgraph import connected_components

# label -> (axis, sign): normals pointing -Y are the front of the character
DIRS = np.array([
    [0, -1, 0],  # 0 front
    [0, 1, 0],   # 1 back
    [1, 0, 0],   # 2 +X (character's left)
    [-1, 0, 0],  # 3 -X
    [0, 0, 1],   # 4 up
    [0, 0, -1],  # 5 down
], np.float64)
# bias: prefer front/back/side over up/down so shoulders and tops merge into the big islands
BIAS = np.array([1.0, 1.0, 0.97, 0.97, 0.80, 0.80])


def _project(label, p):
    """UV (in metres) of points ``p`` (N,3) for projection ``label``."""
    x, y, z = p[:, 0], p[:, 1], p[:, 2]
    if label == 0:
        return np.stack([x, z], 1)
    if label == 1:
        return np.stack([-x, z], 1)
    if label == 2:
        return np.stack([y, z], 1)
    if label == 3:
        return np.stack([-y, z], 1)
    if label == 4:
        return np.stack([x, y], 1)
    return np.stack([x, -y], 1)


def face_adjacency(f):
    """Pairs of faces sharing an edge: (A, B) index arrays plus the shared edge length lookups are done by caller."""
    n = len(f)
    e = np.concatenate([f[:, [0, 1]], f[:, [1, 2]], f[:, [2, 0]]])
    fid = np.tile(np.arange(n), 3)
    key = np.sort(e, axis=1)
    key = key[:, 0].astype(np.int64) * (f.max() + 1) + key[:, 1]
    order = np.argsort(key, kind="stable")
    key, fid, e = key[order], fid[order], e[order]
    same = key[1:] == key[:-1]
    a = fid[:-1][same]
    b = fid[1:][same]
    ea = e[:-1][same]
    return a, b, ea


def _components(label, a, b, nfaces):
    """Connected components of faces that are adjacent AND share a label."""
    m = label[a] == label[b]
    g = coo_matrix((np.ones(m.sum()), (a[m], b[m])), shape=(nfaces, nfaces))
    return connected_components(g, directed=False)


def _geo_normals(v, f):
    t = v[f].astype(np.float64)
    n = np.cross(t[:, 1] - t[:, 0], t[:, 2] - t[:, 0])
    return n / np.maximum(np.linalg.norm(n, axis=1, keepdims=True), 1e-12)


def _fix_flips(v, f, label, nbr, iters=6):
    """Faces whose own normal points away from their projection axis would be mirrored in UV space; move them to a
    neighbouring island that can project them correctly."""
    gn = _geo_normals(v, f)
    dots = gn @ DIRS.T  # (N, 6)
    for _ in range(iters):
        bad = np.where(dots[np.arange(len(f)), label] <= 0.05)[0]
        if len(bad) == 0:
            break
        moved = 0
        new = label.copy()
        for i in bad:
            cands = {int(label[j]) for j in nbr[i]} | {int(dots[i].argmax())}
            best = max(cands, key=lambda c: dots[i, c])
            if dots[i, best] > dots[i, label[i]]:
                new[i] = best
                moved += 1
        label = new
        if not moved:
            break
    return label


def build_labels(v, f, normals_at, smooth_iters=6, min_faces=40):
    n = len(f)
    c = v[f].mean(axis=1)
    fn = normals_at(c)  # smoothed normals at face centroids
    score = (fn @ DIRS.T) * BIAS
    label = score.argmax(axis=1)
    a, b, ea = face_adjacency(f)
    # neighbour lists (up to 3 per face)
    nbr = [[] for _ in range(n)]
    for i, j in zip(a.tolist(), b.tolist()):
        nbr[i].append(j)
        nbr[j].append(i)
    for _ in range(smooth_iters):
        changed = 0
        new = label.copy()
        for i in range(n):
            nb = nbr[i]
            if not nb:
                continue
            ls = label[nb]
            if (ls == label[i]).sum() == 0:
                new[i] = np.bincount(ls).argmax()
                changed += 1
            elif len(nb) >= 3:
                # a face is also moved when it is outvoted and its own score is not decisively better
                cnt = np.bincount(ls, minlength=6)
                best = cnt.argmax()
                if cnt[best] >= 2 and best != label[i] and cnt[label[i]] == 0:
                    new[i] = best
                    changed += 1
        label = new
        if not changed:
            break
    for _round in range(3):
        label = _fix_flips(v, f, label, nbr)
        # merge tiny islands into the neighbouring island that shares the longest border
        for _ in range(8):
            ncomp, comp = _components(label, a, b, n)
            sizes = np.bincount(comp, minlength=ncomp)
            small = np.where(sizes < min_faces)[0]
            if len(small) == 0:
                break
            d = comp[a] != comp[b]
            ca, cb = comp[a][d], comp[b][d]
            edge_len = np.linalg.norm(v[ea[d][:, 0]] - v[ea[d][:, 1]], axis=1)
            for s_ in small:
                sel = (ca == s_) | (cb == s_)
                if not sel.any():
                    continue
                other = np.where(ca[sel] == s_, cb[sel], ca[sel])
                tot = {}
                for o, L in zip(other, edge_len[sel]):
                    tot[o] = tot.get(o, 0.0) + L
                target = max(tot, key=tot.get)
                label[comp == s_] = label[np.where(comp == target)[0][0]]
    return label


def lscm(p3, tris, uv_ref):
    """Least-squares conformal map of one island.  ``p3``: (n,3) positions, ``tris``: (m,3) local indices,
    ``uv_ref``: (n,2) planar projection (used for the two pinned vertices and the orientation).  Returns (n,2) in
    metres, scaled to preserve the island's total area."""
    from scipy.sparse import coo_matrix
    from scipy.sparse.linalg import spsolve

    n = len(p3)
    P = p3.astype(np.float64)
    t0, t1, t2 = P[tris[:, 0]], P[tris[:, 1]], P[tris[:, 2]]
    e1, e2 = t1 - t0, t2 - t0
    L1 = np.linalg.norm(e1, axis=1)
    xa = e1 / np.maximum(L1, 1e-12)[:, None]
    nn = np.cross(e1, e2)
    area = 0.5 * np.linalg.norm(nn, axis=1)
    nn /= np.maximum(np.linalg.norm(nn, axis=1, keepdims=True), 1e-12)
    ya = np.cross(nn, xa)
    q = np.zeros((len(tris), 3, 2))
    q[:, 1, 0] = L1
    q[:, 2, 0] = (e2 * xa).sum(1)
    q[:, 2, 1] = (e2 * ya).sum(1)
    w = np.sqrt(np.maximum(area, 1e-14))
    rows, cols, vals = [], [], []
    for j in range(3):
        j1, j2 = (j + 1) % 3, (j + 2) % 3
        a = (q[:, j1, 1] - q[:, j2, 1]) / (2 * w)    # dy of the opposite edge
        b = -(q[:, j1, 0] - q[:, j2, 0]) / (2 * w)
        r = np.arange(len(tris))
        vid = tris[:, j]
        # row 2r: a*u - b*v ; row 2r+1: b*u + a*v   (unknowns: u = 0..n-1, v = n..2n-1)
        rows += [2 * r, 2 * r, 2 * r + 1, 2 * r + 1]
        cols += [vid, vid + n, vid, vid + n]
        vals += [a, -b, b, a]
    A = coo_matrix((np.concatenate(vals), (np.concatenate(rows), np.concatenate(cols))), shape=(2 * len(tris), 2 * n)).tocsc()
    # pin the two reference vertices that are furthest apart along the principal axis of the planar projection
    c = uv_ref - uv_ref.mean(0)
    axis = np.linalg.svd(c, full_matrices=False)[2][0]
    proj = c @ axis
    i0, i1 = int(proj.argmin()), int(proj.argmax())
    if i0 == i1:
        return uv_ref
    pins = [i0, i1]
    pinned = np.zeros(2 * n, bool)
    pinned[[i0, i1, i0 + n, i1 + n]] = True
    x = np.zeros(2 * n)
    x[[i0, i1]] = uv_ref[[i0, i1], 0]
    x[[i0 + n, i1 + n]] = uv_ref[[i0, i1], 1]
    free = ~pinned
    Af = A[:, free]
    rhs = -(A[:, pinned] @ x[pinned])
    M = (Af.T @ Af).tocsc() + 1e-12 * __import__("scipy.sparse", fromlist=["identity"]).identity(int(free.sum()), format="csc")
    sol = spsolve(M, Af.T @ rhs)
    x[free] = sol
    out = np.stack([x[:n], x[n:]], axis=1)
    # keep the original texel density: rescale about the centroid so total UV area == total 3D area
    a2 = out[tris[:, 1]] - out[tris[:, 0]]
    b2 = out[tris[:, 2]] - out[tris[:, 0]]
    uv_area = 0.5 * np.abs(a2[:, 0] * b2[:, 1] - a2[:, 1] * b2[:, 0]).sum()
    if uv_area > 1e-12:
        out = (out - out.mean(0)) * np.sqrt(area.sum() / uv_area) + out.mean(0)
    return out


def unwrap(v, f, normals_at, resolution=1024, padding=5, min_faces=40, texels_per_meter=None):
    """Return (vmap, faces, uv): new vertex i takes position/attributes of old vertex ``vmap[i]``."""
    import xatlas

    label = build_labels(v, f, normals_at, min_faces=min_faces)
    a, b, _ = face_adjacency(f)
    ncomp, comp = _components(label, a, b, len(f))

    # split vertices per component and project
    key = f.astype(np.int64) * ncomp + comp[:, None]
    uniq, inv = np.unique(key.ravel(), return_inverse=True)
    new_f = inv.reshape(-1, 3).astype(np.int32)
    old_v = (uniq // ncomp).astype(np.int64)
    vcomp = (uniq % ncomp).astype(np.int64)
    uv = np.zeros((len(uniq), 2), np.float64)
    comp_label = np.zeros(ncomp, np.int64)
    comp_label[comp] = label
    face_comp = comp  # component of every face
    for cid in range(ncomp):
        sel = vcomp == cid
        if sel.any():
            uv[sel] = _project(int(comp_label[cid]), v[old_v[sel]].astype(np.float64))
    # unfold every island conformally (planar projection folds over on armpits / undersides)
    for cid in range(ncomp):
        fsel = np.where(face_comp == cid)[0]
        if len(fsel) < 6:
            continue
        vids = np.unique(new_f[fsel])
        remap = {int(g): i for i, g in enumerate(vids)}
        loc = np.vectorize(remap.get)(new_f[fsel])
        try:
            out = lscm(v[old_v[vids]], loc, uv[vids])
        except Exception:
            continue
        # keep it only if it really reduced the number of folded triangles
        def folds(u):
            a_ = u[loc[:, 1]] - u[loc[:, 0]]
            b_ = u[loc[:, 2]] - u[loc[:, 0]]
            sgn = a_[:, 0] * b_[:, 1] - a_[:, 1] * b_[:, 0]
            return min((sgn > 0).sum(), (sgn < 0).sum())
        if np.isfinite(out).all() and folds(out) <= folds(uv[vids]):
            uv[vids] = out

    # xatlas works in texel units for UV meshes: scale metres -> texels so that the islands fill ~60% of the atlas
    a2 = uv[new_f[:, 1]] - uv[new_f[:, 0]]
    b2 = uv[new_f[:, 2]] - uv[new_f[:, 0]]
    total_area = 0.5 * np.abs(a2[:, 0] * b2[:, 1] - a2[:, 1] * b2[:, 0]).sum()
    scale = texels_per_meter or np.sqrt(0.62 * resolution * resolution / total_area)
    for _ in range(14):
        atlas = xatlas.Atlas()
        atlas.add_uv_mesh((uv * scale).astype(np.float32), new_f.astype(np.uint32))
        po = xatlas.PackOptions()
        po.resolution = resolution
        po.padding = padding
        po.bilinear = True
        po.rotate_charts = True
        po.texels_per_unit = 1.0
        atlas.generate(xatlas.ChartOptions(), po, verbose=False)
        if atlas.atlas_count == 1 and atlas.width <= resolution and atlas.height <= resolution:
            break
        scale *= 0.94
    vmapping, indices, out_uv = atlas[0]
    vmap = old_v[vmapping]
    a3 = out_uv[indices[:, 1]] - out_uv[indices[:, 0]]
    b3 = out_uv[indices[:, 2]] - out_uv[indices[:, 0]]
    flipped = int(((a3[:, 0] * b3[:, 1] - a3[:, 1] * b3[:, 0]) < 0).sum())
    stats = dict(islands=int(ncomp), charts=int(atlas.chart_count), flipped=flipped, size=(atlas.width, atlas.height),
                 utilization=float(atlas.utilization), texels_per_meter=float(scale))
    return vmap.astype(np.int64), indices.astype(np.int32), out_uv.astype(np.float32), stats
