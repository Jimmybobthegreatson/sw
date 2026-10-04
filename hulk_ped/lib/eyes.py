"""Eyeballs: UV spheres with a procedural iris texture."""
import numpy as np


def eyeball_mesh(center, radius=0.0135, seg_u=24, seg_v=16, forward=(0.0, -1.0, 0.0)):
    """Returns (verts, tris, uv, normals) for one eyeball.  The iris faces -Y (the character's front)."""
    verts, uvs, nrm = [], [], []
    for j in range(seg_v + 1):
        th = np.pi * j / seg_v                      # 0 at the front pole
        for i in range(seg_u):
            ph = 2 * np.pi * i / seg_u
            d = np.array([np.sin(th) * np.cos(ph), -np.cos(th), np.sin(th) * np.sin(ph)])
            verts.append(np.asarray(center) + d * radius)
            nrm.append(d)
            # planar disc projection around the front pole: the iris sits at the centre of the texture
            rr = np.sin(th) if th < np.pi / 2 else 1.0
            uvs.append((0.5 + 0.5 * rr * np.cos(ph), 0.5 + 0.5 * rr * np.sin(ph)))
    tris = []
    for j in range(seg_v):
        for i in range(seg_u):
            a, b = j * seg_u + i, j * seg_u + (i + 1) % seg_u
            c, d = a + seg_u, b + seg_u
            if j > 0:
                tris.append((a, c, b))
            if j < seg_v - 1:
                tris.append((b, c, d))
    return (np.array(verts, np.float32), np.array(tris, np.int32), np.array(uvs, np.float32), np.array(nrm, np.float32))


def eye_texture(size=128, seed=2):
    """Greenish-hazel iris with a dark limbal ring and faint red veins in the sclera (sRGB 0..1)."""
    rng = np.random.default_rng(seed)
    yy, xx = np.mgrid[0:size, 0:size]
    x = (xx + 0.5) / size * 2 - 1
    y = (yy + 0.5) / size * 2 - 1
    r = np.sqrt(x * x + y * y)
    ang = np.arctan2(y, x)
    img = np.zeros((size, size, 3), np.float32)
    sclera = np.array([0.86, 0.86, 0.78])
    veins = np.clip(np.sin(ang * 23 + 4 * np.sin(ang * 5)) * 0.5 + 0.5, 0, 1) ** 8 * np.clip((r - 0.55) * 2.2, 0, 1)
    img[:] = sclera * (1 - 0.35 * veins[..., None]) + np.array([0.7, 0.25, 0.2]) * 0.35 * veins[..., None]
    iris_r, pupil_r = 0.40, 0.15
    fib = 0.5 + 0.5 * np.sin(ang * 40 + rng.random() * 6) * np.sin(ang * 13)
    iris = np.array([0.30, 0.46, 0.14])[None, None, :] * (0.85 + 0.18 * fib[..., None])
    iris = iris * (1.0 - 0.45 * np.clip((r - 0.3) / 0.1, 0, 1))[..., None] + np.array([0.55, 0.5, 0.15]) * 0.3 * np.clip(1 - r / 0.3, 0, 1)[..., None]
    m_iris = np.clip((iris_r - r) / 0.012, 0, 1)[..., None]
    img = img * (1 - m_iris) + iris * m_iris
    ring = np.clip(1 - np.abs(r - iris_r + 0.015) / 0.03, 0, 1)[..., None]
    img = img * (1 - 0.7 * ring) + np.array([0.02, 0.05, 0.02]) * 0.7 * ring
    m_pupil = np.clip((pupil_r - r) / 0.01, 0, 1)[..., None]
    img = img * (1 - m_pupil) + np.array([0.01, 0.01, 0.01]) * m_pupil
    return np.clip(img, 0, 1)
