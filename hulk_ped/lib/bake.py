"""Procedural texture baking straight from the signed-distance sculpt (numpy only, no GPU / Cycles needed).

For every part the low-poly mesh is rasterised into its UV layout; each texel knows its 3D position, so
ambient occlusion, tangent-space normals (from the high-resolution SDF gradient) and procedural colour can be
evaluated analytically.  Padding is filled with nearest-texel dilation.
"""
import numpy as np
from scipy.ndimage import distance_transform_edt, gaussian_filter, map_coordinates


# --------------------------------------------------------------------------------------------------------------------
# rasterisation
# --------------------------------------------------------------------------------------------------------------------
def rasterize(uv, faces, attrs, res):
    """Rasterise triangles into ``res`` x ``res`` maps.  ``attrs``: dict name -> (V, k) per-vertex arrays.

    Image row 0 is the top (v = 1).  Returns (maps dict name -> (res, res, k), mask (res, res) bool,
    tri_id (res, res) int32).
    """
    out = {k: np.zeros((res, res, a.shape[1]), np.float32) for k, a in attrs.items()}
    mask = np.zeros((res, res), bool)
    tri_id = np.full((res, res), -1, np.int32)
    px = uv[:, 0] * res
    py = (1.0 - uv[:, 1]) * res
    for t, (a, b, c) in enumerate(faces):
        x0, y0, x1, y1, x2, y2 = px[a], py[a], px[b], py[b], px[c], py[c]
        den = (y1 - y2) * (x0 - x2) + (x2 - x1) * (y0 - y2)
        if abs(den) < 1e-12:
            continue
        xmin = max(int(np.floor(min(x0, x1, x2) - 0.5)), 0)
        xmax = min(int(np.ceil(max(x0, x1, x2) + 0.5)), res - 1)
        ymin = max(int(np.floor(min(y0, y1, y2) - 0.5)), 0)
        ymax = min(int(np.ceil(max(y0, y1, y2) + 0.5)), res - 1)
        if xmax < xmin or ymax < ymin:
            continue
        xs, ys = np.meshgrid(np.arange(xmin, xmax + 1) + 0.5, np.arange(ymin, ymax + 1) + 0.5)
        l0 = ((y1 - y2) * (xs - x2) + (x2 - x1) * (ys - y2)) / den
        l1 = ((y2 - y0) * (xs - x2) + (x0 - x2) * (ys - y2)) / den
        l2 = 1.0 - l0 - l1
        inside = (l0 >= -1e-4) & (l1 >= -1e-4) & (l2 >= -1e-4)
        if not inside.any():
            continue
        iy, ix = np.nonzero(inside)
        w0, w1, w2 = l0[inside], l1[inside], l2[inside]
        yy, xx = iy + ymin, ix + xmin
        for k, arr in attrs.items():
            out[k][yy, xx] = w0[:, None] * arr[a] + w1[:, None] * arr[b] + w2[:, None] * arr[c]
        mask[yy, xx] = True
        tri_id[yy, xx] = t
    return out, mask, tri_id


def dilate(img, mask):
    """Fill texels outside ``mask`` with the nearest valid texel (padding for mip-mapping / filtering)."""
    _, (iy, ix) = distance_transform_edt(~mask, return_indices=True)
    return img[iy, ix]


# --------------------------------------------------------------------------------------------------------------------
# noise
# --------------------------------------------------------------------------------------------------------------------
class Noise3:
    """Cheap tileable 3D value noise (cubic interpolation of a random lattice)."""

    def __init__(self, seed=0, n=48):
        self.r = np.random.default_rng(seed).random((n, n, n)).astype(np.float32)

    def __call__(self, p, freq):
        q = (np.asarray(p, np.float64) * freq).T
        return map_coordinates(self.r, q, order=3, mode="grid-wrap", prefilter=False).astype(np.float32)

    def fbm(self, p, freq, octaves=4, gain=0.5):
        total, amp, norm = 0.0, 1.0, 0.0
        for o in range(octaves):
            total = total + amp * self(p + o * 13.7, freq * (2.0 ** o))
            norm += amp
            amp *= gain
        return total / norm

    def ridged(self, p, freq, octaves=3):
        """1 on thin ridges (vein-like lines), 0 elsewhere."""
        n = self.fbm(p, freq, octaves)
        return 1.0 - np.abs(2.0 * n - 1.0)


def smoothstep(a, b, x):
    t = np.clip((x - a) / (b - a), 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


def srgb_hex(r, g, b):
    return np.array([r, g, b], np.float32) / 255.0


# --------------------------------------------------------------------------------------------------------------------
# geometry maps
# --------------------------------------------------------------------------------------------------------------------
def ambient_occlusion(grid, pos, nrm, strength=1.0):
    """Classic SDF ambient occlusion: how much closer than expected is the nearest surface along the normal."""
    occ = np.zeros(len(pos), np.float32)
    w = 1.0
    for h in (0.006, 0.014, 0.03, 0.06, 0.11):
        d = grid.sample(pos + nrm * h)
        occ += w * np.clip(h - d, 0.0, None) / h
        w *= 0.62
    return np.clip(1.0 - strength * occ * 0.55, 0.0, 1.0)


def tangent_frames(v, f, uv, vn):
    """Per-vertex tangent (dP/du) and bitangent (dP/dv) averaged from the triangles, orthogonalised against vn."""
    t = np.zeros_like(v, dtype=np.float64)
    b = np.zeros_like(v, dtype=np.float64)
    p0, p1, p2 = v[f[:, 0]].astype(np.float64), v[f[:, 1]].astype(np.float64), v[f[:, 2]].astype(np.float64)
    u0, u1, u2 = uv[f[:, 0]], uv[f[:, 1]], uv[f[:, 2]]
    e1, e2 = p1 - p0, p2 - p0
    d1, d2 = u1 - u0, u2 - u0
    det = d1[:, 0] * d2[:, 1] - d2[:, 0] * d1[:, 1]
    det = np.where(np.abs(det) < 1e-14, 1e-14, det)
    tt = (e1 * d2[:, 1:2] - e2 * d1[:, 1:2]) / det[:, None]
    bb = (e2 * d1[:, 0:1] - e1 * d2[:, 0:1]) / det[:, None]
    area = np.linalg.norm(np.cross(e1, e2), axis=1)[:, None]
    # normalise per triangle so tiny triangles don't dominate
    tt /= np.maximum(np.linalg.norm(tt, axis=1, keepdims=True), 1e-12)
    bb /= np.maximum(np.linalg.norm(bb, axis=1, keepdims=True), 1e-12)
    for k in range(3):
        np.add.at(t, f[:, k], tt * area)
        np.add.at(b, f[:, k], bb * area)
    n = vn.astype(np.float64)
    t = t - n * (t * n).sum(1, keepdims=True)
    b = b - n * (b * n).sum(1, keepdims=True)
    t /= np.maximum(np.linalg.norm(t, axis=1, keepdims=True), 1e-12)
    b /= np.maximum(np.linalg.norm(b, axis=1, keepdims=True), 1e-12)
    return t.astype(np.float32), b.astype(np.float32)


class PartMaps:
    """Rasterised geometry of a part: positions, smooth normals, frames and a validity mask."""

    def __init__(self, v, f, uv, vn, res):
        t, b = tangent_frames(v, f, uv, vn)
        maps, self.mask, self.tri = rasterize(uv, f, {"pos": v, "nrm": vn, "tan": t, "bit": b}, res)
        self.res = res
        m = self.mask
        self.pos = maps["pos"][m]
        self.nrm = _unit(maps["nrm"][m])
        self.tan = maps["tan"][m]
        self.bit = maps["bit"][m]
        # re-orthogonalise after interpolation
        self.tan = _unit(self.tan - self.nrm * (self.tan * self.nrm).sum(1, keepdims=True))
        self.bit = _unit(self.bit - self.nrm * (self.bit * self.nrm).sum(1, keepdims=True))

    def project(self, grid, iters=3, max_step=0.03):
        """Snap the texel positions onto the high-resolution surface (Newton steps on the SDF); also stores the
        true surface normal there.  Everything procedural is then evaluated on the real sculpt surface."""
        p = self.pos.astype(np.float64)
        for _ in range(iters):
            d = grid.sample(p)
            g = grid.gradient(p, eps=max(grid.h * 1.1, 0.003))
            step = np.clip(d, -max_step, max_step)[:, None] * g
            p = p - step
        self.spos = p.astype(np.float32)
        self.snrm = grid.gradient(p, eps=max(grid.h * 1.1, 0.003)).astype(np.float32)

    def to_image(self, values, fill=0.0):
        k = values.shape[1] if values.ndim > 1 else 1
        img = np.zeros((self.res, self.res, k), np.float32) + fill
        img[self.mask] = values.reshape(-1, k)
        return dilate(img, self.mask)


def _unit(a):
    return a / np.maximum(np.linalg.norm(a, axis=1, keepdims=True), 1e-12)


def normal_map(pm, grid, detail=None, detail_strength=0.0, flip_green=False):
    """Tangent-space normal map from the high-resolution SDF gradient, plus optional micro detail ``(du, dv)``."""
    nh = pm.snrm
    x = (nh * pm.tan).sum(1)
    y = (nh * pm.bit).sum(1)
    z = (nh * pm.nrm).sum(1)
    # limit the tilt (bad texels across creases of the decimated mesh would otherwise turn black)
    zmin = np.cos(np.radians(58.0))
    lim = np.maximum(z, zmin)
    sc = np.minimum(1.0, np.sqrt(np.maximum(1.0 - lim * lim, 1e-9)) / np.maximum(np.sqrt(x * x + y * y), 1e-9))
    n = np.stack([x * sc, y * sc, lim], axis=1)
    if detail is not None:
        n[:, 0] += detail_strength * detail[0]
        n[:, 1] += detail_strength * detail[1]
    n = _unit(n)
    if flip_green:  # DirectX convention used by GTA V (Sollumz inverts the green channel for display)
        n[:, 1] = -n[:, 1]
    return pm.to_image(n * 0.5 + 0.5, fill=0.5)


def micro_detail(res, sigma=1.1, seed=3):
    """Texel-space bump noise -> gradient pair for normal perturbation."""
    h = gaussian_filter(np.random.default_rng(seed).standard_normal((res, res)).astype(np.float32), sigma)
    h /= h.std() + 1e-6
    gy, gx = np.gradient(h)
    return gx, gy
