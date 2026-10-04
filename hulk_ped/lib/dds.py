"""Minimal DDS writer: DXT1 (BC1) / DXT5 (BC3) with a full mip chain, numpy only.

GTA V reads standard DXT-compressed DDS files.  The compressor is a classic bounding-box endpoint fit, which is
plenty for skin / denim noise and tangent-space normal maps.
"""
import struct

import numpy as np


def _to565(c):
    c = np.clip(np.rint(c), 0, 255).astype(np.uint16)
    return ((c[..., 0] >> 3) << 11) | ((c[..., 1] >> 2) << 5) | (c[..., 2] >> 3)


def _from565(v):
    r = ((v >> 11) & 31).astype(np.float32) * 255.0 / 31.0
    g = ((v >> 5) & 63).astype(np.float32) * 255.0 / 63.0
    b = (v & 31).astype(np.float32) * 255.0 / 31.0
    return np.stack([r, g, b], axis=-1)


def _blocks(img):
    h, w = img.shape[:2]
    return img.reshape(h // 4, 4, w // 4, 4, -1).transpose(0, 2, 1, 3, 4).reshape(h // 4, w // 4, 16, -1)


def _encode_color(rgb):
    """rgb: (H, W, 3) float 0..255 (H, W multiples of 4) -> bytes of BC1 colour blocks, shape (nb, 8)."""
    blk = _blocks(rgb)  # (bh, bw, 16, 3)
    lo = blk.min(axis=2)
    hi = blk.max(axis=2)
    inset = (hi - lo) / 16.0
    lo = lo + inset
    hi = hi - inset
    c0 = _to565(hi)
    c1 = _to565(lo)
    swap = c0 < c1
    c0, c1 = np.where(swap, c1, c0), np.where(swap, c0, c1)
    equal = c0 == c1
    p0, p1 = _from565(c0), _from565(c1)
    axis = p0 - p1
    denom = np.maximum((axis * axis).sum(-1), 1e-6)
    t = ((blk - p1[:, :, None, :]) * axis[:, :, None, :]).sum(-1) / denom[:, :, None]
    q = np.clip(np.rint(t * 3.0), 0, 3).astype(np.uint8)  # 0 -> c1, 3 -> c0
    lut = np.array([1, 3, 2, 0], np.uint8)
    idx = lut[q]
    idx = np.where(equal[:, :, None], 0, idx).astype(np.uint32)
    shifts = (2 * np.arange(16)).astype(np.uint32)
    bits = (idx << shifts).sum(axis=2, dtype=np.uint32)
    out = np.empty(c0.shape + (8,), np.uint8)
    out[..., 0:2] = c0.astype("<u2").view(np.uint8).reshape(c0.shape + (2,))
    out[..., 2:4] = c1.astype("<u2").view(np.uint8).reshape(c0.shape + (2,))
    out[..., 4:8] = bits.astype("<u4").view(np.uint8).reshape(c0.shape + (4,))
    return out


def _encode_alpha(a):
    """a: (H, W) float 0..255 -> BC3 alpha blocks (bh, bw, 8)."""
    blk = _blocks(a[..., None])[..., 0]  # (bh, bw, 16)
    a0 = np.clip(np.rint(blk.max(axis=2)), 0, 255)
    a1 = np.clip(np.rint(blk.min(axis=2)), 0, 255)
    flat = a0 == a1
    # a0 > a1 -> 8 interpolated values: a0, a1, (6a0+1a1)/7 ... (1a0+6a1)/7
    pal = np.stack([a0, a1] + [((7 - i) * a0 + i * a1) / 7.0 for i in range(1, 7)], axis=-1)  # (bh,bw,8)
    d = np.abs(blk[..., None] - pal[:, :, None, :])
    idx = d.argmin(axis=-1).astype(np.uint64)
    idx = np.where(flat[:, :, None], 0, idx).astype(np.uint64)
    shifts = (3 * np.arange(16)).astype(np.uint64)
    bits = (idx << shifts).sum(axis=2, dtype=np.uint64)
    out = np.empty(a0.shape + (8,), np.uint8)
    out[..., 0] = a0.astype(np.uint8)
    out[..., 1] = a1.astype(np.uint8)
    out[..., 2:8] = bits.astype("<u8").view(np.uint8).reshape(a0.shape + (8,))[..., :6]
    return out


def _downsample(img):
    h, w = img.shape[:2]
    return img.reshape(h // 2, 2, w // 2, 2, -1).mean(axis=(1, 3))


def write_dds(path, rgba, alpha=False, mips=True):
    """``rgba``: float (H, W, 4) in 0..1 (sRGB-encoded values).  Writes DXT5 if ``alpha`` else DXT1."""
    img = np.clip(np.asarray(rgba, np.float32), 0, 1) * 255.0
    h, w = img.shape[:2]
    assert h % 4 == 0 and w % 4 == 0 and (h & (h - 1)) == 0 and (w & (w - 1)) == 0, "power-of-two size required"
    levels = []
    cur = img
    while True:
        levels.append(cur)
        if not mips or (cur.shape[0] <= 4 and cur.shape[1] <= 4):
            break
        cur = _downsample(cur) if cur.shape[0] > 1 and cur.shape[1] > 1 else cur
        if cur.shape[0] < 4 or cur.shape[1] < 4:
            break
    payload = []
    for lv in levels:
        lh, lw = lv.shape[:2]
        col = _encode_color(lv[..., :3]).reshape(lh // 4, lw // 4, 8)
        if alpha:
            al = _encode_alpha(lv[..., 3])
            payload.append(np.concatenate([al, col], axis=-1).tobytes())
        else:
            payload.append(col.tobytes())
    block = 16 if alpha else 8
    linear = max(1, w // 4) * max(1, h // 4) * block
    flags = 0x1 | 0x2 | 0x4 | 0x1000 | 0x20000 | 0x80000  # CAPS HEIGHT WIDTH PIXELFORMAT MIPMAPCOUNT LINEARSIZE
    hdr = struct.pack("<4sI", b"DDS ", 124)
    hdr += struct.pack("<6I", flags, h, w, linear, 0, len(levels))
    hdr += b"\0" * 44
    hdr += struct.pack("<2I4s5I", 32, 0x4, b"DXT5" if alpha else b"DXT1", 0, 0, 0, 0, 0)
    hdr += struct.pack("<5I", 0x1000 | (0x400000 if len(levels) > 1 else 0) | (0x8 if len(levels) > 1 else 0), 0, 0, 0, 0)
    with open(path, "wb") as f:
        f.write(hdr + b"".join(payload))
    return path
