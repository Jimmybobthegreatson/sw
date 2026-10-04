"""Tiny signed-distance-field modelling kit (numpy only).

A :class:`Grid` holds a dense float32 distance volume.  Primitives are blended into it with polynomial smooth-min
so that separate muscle masses melt into one organic surface, and are evaluated only inside their bounding box so
that hundreds of primitives stay cheap.  Negative = inside.  Units are metres.
"""
import numpy as np
from skimage import measure


# --------------------------------------------------------------------------------------------------------------------
# rotations
# --------------------------------------------------------------------------------------------------------------------
def rot_x(deg):
    c, s = np.cos(np.radians(deg)), np.sin(np.radians(deg))
    return np.array([[1, 0, 0], [0, c, -s], [0, s, c]], np.float32)


def rot_y(deg):
    c, s = np.cos(np.radians(deg)), np.sin(np.radians(deg))
    return np.array([[c, 0, s], [0, 1, 0], [-s, 0, c]], np.float32)


def rot_z(deg):
    c, s = np.cos(np.radians(deg)), np.sin(np.radians(deg))
    return np.array([[c, -s, 0], [s, c, 0], [0, 0, 1]], np.float32)


def euler(rx=0.0, ry=0.0, rz=0.0):
    """Rotation applied X, then Y, then Z (degrees)."""
    return rot_z(rz) @ rot_y(ry) @ rot_x(rx)


def align_z(axis, up=(0.0, 0.0, 1.0)):
    """Rotation whose local +Z is ``axis`` (local +X is as close to ``up x axis`` as possible)."""
    z = np.asarray(axis, np.float64)
    z = z / np.linalg.norm(z)
    up = np.asarray(up, np.float64)
    if abs(np.dot(z, up)) > 0.98:
        up = np.array([0.0, 1.0, 0.0])
    x = np.cross(up, z)
    x /= np.linalg.norm(x)
    y = np.cross(z, x)
    return np.stack([x, y, z], axis=1).astype(np.float32)


MIRROR = np.diag([-1.0, 1.0, 1.0]).astype(np.float32)


def mirror_pt(p):
    return (-p[0], p[1], p[2])


def mirror_rot(r):
    return None if r is None else (MIRROR @ r @ MIRROR).astype(np.float32)


# --------------------------------------------------------------------------------------------------------------------
# smooth booleans
# --------------------------------------------------------------------------------------------------------------------
def smin(a, b, k):
    if k <= 0:
        return np.minimum(a, b)
    h = np.clip(0.5 + 0.5 * (b - a) / k, 0.0, 1.0)
    return b + (a - b) * h - k * h * (1.0 - h)


def smax(a, b, k):
    return -smin(-a, -b, k)


# --------------------------------------------------------------------------------------------------------------------
# grid
# --------------------------------------------------------------------------------------------------------------------
class Grid:
    def __init__(self, lo, hi, h):
        self.h = float(h)
        self.lo = np.asarray(lo, np.float64)
        n = np.ceil((np.asarray(hi, np.float64) - self.lo) / self.h).astype(int) + 1
        self.shape = tuple(int(v) for v in n)
        self.F = np.full(self.shape, 1.0, np.float32)
        self.axes = [(self.lo[i] + self.h * np.arange(n[i])).astype(np.float32) for i in range(3)]

    @classmethod
    def from_array(cls, F, lo, h):
        g = cls.__new__(cls)
        g.h = float(h)
        g.lo = np.asarray(lo, np.float64)
        g.F = F
        g.shape = F.shape
        g.axes = [(g.lo[i] + g.h * np.arange(F.shape[i])).astype(np.float32) for i in range(3)]
        return g

    # -- helpers ---------------------------------------------------------------------------------------------------
    def window(self, bmin, bmax):
        sl = []
        for i in range(3):
            a = int(max(0, np.floor((bmin[i] - self.lo[i]) / self.h)))
            b = int(min(self.shape[i], np.ceil((bmax[i] - self.lo[i]) / self.h) + 1))
            sl.append(slice(a, max(a, b)))
        ax = self.axes
        P = (ax[0][sl[0]][:, None, None], ax[1][sl[1]][None, :, None], ax[2][sl[2]][None, None, :])
        return tuple(sl), P

    def _apply(self, bmin, bmax, fn, k, op):
        bmin = np.asarray(bmin, np.float64) - k - self.h
        bmax = np.asarray(bmax, np.float64) + k + self.h
        sl, P = self.window(bmin, bmax)
        if any(s.stop <= s.start for s in sl):
            return
        d = fn(P)
        cur = self.F[sl]
        if op == "add":
            self.F[sl] = smin(cur, d, k)
        elif op == "sub":
            self.F[sl] = smax(cur, -d, k)
        elif op == "intersect":
            self.F[sl] = smax(cur, d, k)
        else:
            raise ValueError(op)

    # -- primitives ------------------------------------------------------------------------------------------------
    def ellipsoid(self, c, r, rot=None, k=0.0, op="add", sym=False):
        """Ellipsoid with radii ``r`` (local axes), rotated by ``rot`` (local -> world)."""
        for s in ((1, -1) if sym else (1,)):
            cc, ro = (c, rot) if s == 1 else (mirror_pt(c), mirror_rot(rot))
            self._ellipsoid(cc, r, ro, k, op)

    def _ellipsoid(self, c, r, rot, k, op):
        c = np.asarray(c, np.float32)
        r = np.asarray(r, np.float32)
        ext = float(np.max(r)) if rot is not None else None
        bm = (c - (ext if ext else r), c + (ext if ext else r))
        rmin = float(r.min())
        Rt = None if rot is None else np.asarray(rot, np.float32).T

        def fn(P):
            q = [P[0] - c[0], P[1] - c[1], P[2] - c[2]]
            if Rt is not None:
                q = [Rt[i, 0] * q[0] + Rt[i, 1] * q[1] + Rt[i, 2] * q[2] for i in range(3)]
            k0 = np.sqrt((q[0] / r[0]) ** 2 + (q[1] / r[1]) ** 2 + (q[2] / r[2]) ** 2)
            k1 = np.sqrt((q[0] / r[0] ** 2) ** 2 + (q[1] / r[1] ** 2) ** 2 + (q[2] / r[2] ** 2) ** 2) + 1e-9
            return k0 * (k0 - 1.0) / k1

        self._apply(bm[0], bm[1], fn, k, op)

    def capsule(self, a, b, ra, rb=None, k=0.0, op="add", sym=False):
        """Round cone between points ``a`` and ``b`` with end radii ``ra`` / ``rb``."""
        rb = ra if rb is None else rb
        for s in ((1, -1) if sym else (1,)):
            aa, bb = (a, b) if s == 1 else (mirror_pt(a), mirror_pt(b))
            self._capsule(aa, bb, ra, rb, k, op)

    def _capsule(self, a, b, ra, rb, k, op):
        a = np.asarray(a, np.float32)
        b = np.asarray(b, np.float32)
        ba = b - a
        inv = 1.0 / float(np.dot(ba, ba))
        m = max(ra, rb)

        def fn(P):
            px, py, pz = P[0] - a[0], P[1] - a[1], P[2] - a[2]
            t = np.clip((px * ba[0] + py * ba[1] + pz * ba[2]) * inv, 0.0, 1.0)
            dx, dy, dz = px - ba[0] * t, py - ba[1] * t, pz - ba[2] * t
            return np.sqrt(dx * dx + dy * dy + dz * dz) - (ra + (rb - ra) * t)

        self._apply(np.minimum(a, b) - m, np.maximum(a, b) + m, fn, k, op)

    def tube(self, pts, rx, ry=None, k=0.0, op="add", sym=False, up=(0.0, 1.0, 0.0)):
        """Elliptical cross-section tube through ``pts`` with per-point radii ``rx`` (width) / ``ry`` (depth).

        ``up`` defines the local Y direction of the cross-section (``rx`` is measured along ``up x axis``).
        Segments are joined with a hard union (no smooth-min inflation at the joints); ``k`` only blends the whole
        tube into what is already in the grid.
        """
        ry = rx if ry is None else ry
        for s in ((1, -1) if sym else (1,)):
            p = pts if s == 1 else [mirror_pt(q) for q in pts]
            u = up if s == 1 else mirror_pt(up)
            segs = [self._tube_seg(p[i], p[i + 1], rx[i], rx[i + 1], ry[i], ry[i + 1], u) for i in range(len(p) - 1)]
            bmin = np.min([sg[0] for sg in segs], axis=0)
            bmax = np.max([sg[1] for sg in segs], axis=0)

            def fn(P, segs=segs):
                d = segs[0][2](P)
                for sg in segs[1:]:
                    d = np.minimum(d, sg[2](P))
                return d

            self._apply(bmin, bmax, fn, k, op)

    @staticmethod
    def _tube_seg(a, b, rxa, rxb, rya, ryb, up):
        a = np.asarray(a, np.float32)
        b = np.asarray(b, np.float32)
        ba = b - a
        inv = 1.0 / float(np.dot(ba, ba))
        R = align_z(ba, up)  # columns: local x, y, z
        ex, ey = R[:, 0], R[:, 1]
        m = max(rxa, rxb, rya, ryb)

        def fn(P):
            px, py, pz = P[0] - a[0], P[1] - a[1], P[2] - a[2]
            t = np.clip((px * ba[0] + py * ba[1] + pz * ba[2]) * inv, 0.0, 1.0)
            dx, dy, dz = px - ba[0] * t, py - ba[1] * t, pz - ba[2] * t
            u = dx * ex[0] + dy * ex[1] + dz * ex[2]
            v = dx * ey[0] + dy * ey[1] + dz * ey[2]
            rx = rxa + (rxb - rxa) * t
            ry = rya + (ryb - rya) * t
            q = np.sqrt((u / rx) ** 2 + (v / ry) ** 2)
            along = np.sqrt(np.maximum(dx * dx + dy * dy + dz * dz - u * u - v * v, 0.0))  # >0 only past the ends
            return (q - 1.0) * np.minimum(rx, ry) + along

        return np.minimum(a, b) - m, np.maximum(a, b) + m, fn

    def crease(self, pts, depth, width, sym=False):
        """Dent (``depth`` > 0) or raise (``depth`` < 0) the surface along a polyline, wherever the surface passes
        through the neighbourhood of the curve.  Independent of the exact surface position."""
        for s in ((1, -1) if sym else (1,)):
            p = [np.asarray(q if s == 1 else mirror_pt(q), np.float32) for q in pts]
            lo = np.min(p, axis=0) - 3.2 * width
            hi = np.max(p, axis=0) + 3.2 * width
            sl, P = self.window(lo, hi)
            if any(sx.stop <= sx.start for sx in sl):
                continue
            d = None
            for a, b in zip(p[:-1], p[1:]):
                ba = b - a
                inv = 1.0 / max(float(np.dot(ba, ba)), 1e-12)
                px, py, pz = P[0] - a[0], P[1] - a[1], P[2] - a[2]
                t = np.clip((px * ba[0] + py * ba[1] + pz * ba[2]) * inv, 0.0, 1.0)
                dx, dy, dz = px - ba[0] * t, py - ba[1] * t, pz - ba[2] * t
                dd = dx * dx + dy * dy + dz * dz
                d = dd if d is None else np.minimum(d, dd)
            self.F[sl] += (depth * np.exp(-d / (width * width))).astype(np.float32)

    def box(self, c, half, rot=None, rounding=0.0, k=0.0, op="add", sym=False):
        """Rounded box with half extents ``half``."""
        for s in ((1, -1) if sym else (1,)):
            cc, ro = (c, rot) if s == 1 else (mirror_pt(c), mirror_rot(rot))
            self._box(cc, half, ro, rounding, k, op)

    def _box(self, c, half, rot, rounding, k, op):
        c = np.asarray(c, np.float32)
        half = np.asarray(half, np.float32)
        ext = float(np.linalg.norm(half)) + rounding
        Rt = None if rot is None else np.asarray(rot, np.float32).T

        def fn(P):
            q = [P[0] - c[0], P[1] - c[1], P[2] - c[2]]
            if Rt is not None:
                q = [Rt[i, 0] * q[0] + Rt[i, 1] * q[1] + Rt[i, 2] * q[2] for i in range(3)]
            d = [np.abs(q[i]) - (half[i] - rounding) for i in range(3)]
            out = np.sqrt(sum(np.maximum(v, 0.0) ** 2 for v in d))
            ins = np.minimum(np.maximum(np.maximum(d[0], d[1]), d[2]), 0.0)
            return out + ins - rounding

        self._apply(c - ext, c + ext, fn, k, op)

    def loft(self, sections, k=0.0, op="add", power=2.0):
        """Vertical column through cross-sections.

        ``sections`` = [(z, cx, cy, rx, ry), ...]; values are interpolated with a smooth (PCHIP-like) spline.
        ``power`` > 2 gives superelliptical (boxier) sections.
        """
        from scipy.interpolate import PchipInterpolator

        s = np.asarray(sections, np.float64)
        z0, z1 = s[0, 0], s[-1, 0]
        itp = [PchipInterpolator(s[:, 0], s[:, i]) for i in range(1, 5)]
        pad = float(max(s[:, 3].max(), s[:, 4].max()))
        cxm = float(np.abs(s[:, 1]).max())
        cym = float(s[:, 2].min()), float(s[:, 2].max())

        def fn(P):
            Z = np.clip(P[2], z0, z1)
            cx, cy, rx, ry = (f(Z) for f in itp)
            cx, cy, rx, ry = (np.asarray(v, np.float32) for v in (cx, cy, rx, ry))
            u = (P[0] - cx) / rx
            v = (P[1] - cy) / ry
            if power == 2.0:
                q = np.sqrt(u * u + v * v)
            else:
                q = (np.abs(u) ** power + np.abs(v) ** power) ** (1.0 / power)
            d2 = (q - 1.0) * np.minimum(rx, ry)
            dz = np.maximum(np.maximum(z0 - P[2], P[2] - z1), 0.0)
            return np.sqrt(np.maximum(d2, 0.0) ** 2 + dz * dz) + np.minimum(d2, 0.0)

        self._apply((-cxm - pad, cym[0] - pad, z0 - 0.01), (cxm + pad, cym[1] + pad, z1 + 0.01), fn, k, op)

    def halfspace(self, point, normal, k=0.0, op="intersect"):
        """Keep the side the ``normal`` points *away* from (``intersect``) / remove it (``sub``)."""
        n = np.asarray(normal, np.float32)
        n = n / np.linalg.norm(n)
        p0 = np.asarray(point, np.float32)

        def fn(P):
            return (P[0] - p0[0]) * n[0] + (P[1] - p0[1]) * n[1] + (P[2] - p0[2]) * n[2]

        self._apply(self.lo, self.lo + np.asarray(self.shape) * self.h, fn, k, op)

    # -- surface snapping ------------------------------------------------------------------------------------------
    def snap(self, pts, direction, back=0.35, length=0.8):
        """Project points onto the surface by marching along ``direction`` (pointing from outside to inside).

        Returns (N, 3); rows are NaN where the ray never enters the body.
        """
        pts = np.asarray(pts, np.float64)
        d = np.asarray(direction, np.float64)
        d = d / np.linalg.norm(d, axis=-1, keepdims=True)
        d = np.broadcast_to(d, pts.shape)[:, None, :]  # (N, 1, 3): one direction per point
        t = np.arange(0.0, length, self.h * 0.5)
        P = pts[:, None, :] - d * back + t[None, :, None] * d
        vals = self.sample(P.reshape(-1, 3)).reshape(len(pts), len(t))
        inside = vals < 0
        first = inside.argmax(axis=1)
        hit = inside.any(axis=1) & (first > 0)
        i = np.arange(len(pts))
        i0 = np.maximum(first - 1, 0)
        v0, v1 = vals[i, i0], vals[i, first]
        w = np.where(np.abs(v0 - v1) > 1e-9, v0 / (v0 - v1 + 1e-12), 0.0)
        pos = P[i, i0] + (P[i, first] - P[i, i0]) * w[:, None]
        pos[~hit] = np.nan
        return pos

    def crease_snapped(self, pts, direction, depth, width, sym=False, inset=0.0, step=0.004):
        """Crease along a polyline whose points are projected onto the current surface along ``direction``."""
        for s in ((1, -1) if sym else (1,)):
            p = [q if s == 1 else mirror_pt(q) for q in pts]
            dirv = direction if s == 1 else mirror_pt(direction)
            dense = []
            for a, b in zip(p[:-1], p[1:]):
                a, b = np.asarray(a, np.float64), np.asarray(b, np.float64)
                n = max(2, int(np.ceil(np.linalg.norm(b - a) / step)))
                dense.append(a + (b - a) * (np.arange(n) / n)[:, None])
            dense.append(np.asarray(p[-1], np.float64)[None, :])
            dense = np.vstack(dense)
            hit = self.snap(dense, dirv)
            ok = ~np.isnan(hit[:, 0])
            if ok.sum() < 2:
                continue
            u = np.asarray(dirv, np.float64)
            u = u / np.linalg.norm(u)
            curve = hit[ok] + u * inset
            self.crease([tuple(q) for q in curve], depth, width)

    def crease_ring(self, center, z, rx, ry, depth, width, a0=0.0, a1=360.0, tilt=0.0, n=360, inset=0.0):
        """Crease around the body: points on an ellipse (rx, ry) around ``center`` snapped radially inward.

        ``tilt`` raises the ring by ``tilt * y`` (e.g. a waistband that sits higher at the back)."""
        ang = np.radians(np.linspace(a0, a1, n))
        c = np.asarray(center, np.float64)
        pts = np.stack([c[0] + rx * np.cos(ang), c[1] + ry * np.sin(ang), z + tilt * (ry * np.sin(ang))], axis=1)
        dirs = np.stack([c[0] - pts[:, 0], c[1] - pts[:, 1], np.zeros(n)], axis=1)
        hit = self.snap(pts, dirs, back=0.0, length=max(rx, ry) * 1.2)
        ok = ~np.isnan(hit[:, 0])
        if ok.sum() < 2:
            return
        u = dirs[ok] / np.linalg.norm(dirs[ok], axis=1, keepdims=True)
        curve = hit[ok] + u * inset
        self.crease([tuple(q) for q in curve], depth, width)

    # -- queries ---------------------------------------------------------------------------------------------------
    def sample(self, pts):
        """Trilinear lookup of the field at world points (N, 3)."""
        from scipy.ndimage import map_coordinates

        idx = (np.asarray(pts, np.float64) - self.lo) / self.h
        return map_coordinates(self.F, idx.T, order=1, mode="nearest").astype(np.float32)

    def gradient(self, pts, eps=None):
        eps = eps or self.h
        g = np.zeros((len(pts), 3), np.float32)
        for i in range(3):
            d = np.zeros(3)
            d[i] = eps
            g[:, i] = self.sample(np.asarray(pts) + d) - self.sample(np.asarray(pts) - d)
        n = np.linalg.norm(g, axis=1, keepdims=True)
        return g / np.maximum(n, 1e-9)

    # -- meshing ---------------------------------------------------------------------------------------------------
    def mesh(self, level=0.0):
        """Marching cubes -> (verts (N,3) float32 world coords, faces (M,3) int32), outward-facing."""
        F = np.pad(self.F, 1, constant_values=1.0)
        v, f, _, _ = measure.marching_cubes(F, level=level, spacing=(self.h,) * 3, allow_degenerate=False)
        v = v + self.lo - self.h  # undo the 1-voxel pad
        return v.astype(np.float32), f.astype(np.int32)
