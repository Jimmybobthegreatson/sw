"""Procedural colour for the baked textures.  All colours are sRGB-encoded 0..1; positions are in ped space."""
import numpy as np

from . import anatomy, meshing
from .bake import smoothstep, srgb_hex


def mix(a, b, t):
    t = np.asarray(t, np.float32)
    if t.ndim == 1:
        t = t[:, None]
    return a * (1.0 - t) + b * t


def skin(pos, nrm, ao, N, part):
    """Bright yellow-green Hulk skin with mottling, veins, lips and dark brows."""
    x, y, z = pos[:, 0], pos[:, 1], pos[:, 2]
    dark, light = srgb_hex(98, 154, 34), srgb_hex(170, 214, 62)
    t = np.clip(0.50 + (N.fbm(pos, 4.0, 4) - 0.5) * 1.15, 0, 1)
    col = mix(dark, light, t)
    col = col * (1.0 + 0.07 * nrm[:, 2:3])                      # slightly brighter on up-facing skin
    # veins: ridged noise, strongest on the forearms, biceps, neck and temples
    limb = np.maximum(smoothstep(0.22, 0.38, np.abs(x)) * smoothstep(0.52, 0.30, z), smoothstep(0.5, 0.62, z) * 0.8)
    limb = np.maximum(limb, 0.25)
    vein = smoothstep(0.84, 0.94, N.ridged(pos, 24.0, 3)) * limb
    col = mix(col, srgb_hex(40, 86, 24), 0.55 * vein)
    col = col * (0.93 + 0.14 * N.fbm(pos, 110.0, 2))[:, None]  # fine mottling
    # deep creases go olive
    col = mix(col, srgb_hex(46, 84, 20), 0.55 * smoothstep(0.75, 0.35, ao))
    col = col * (0.62 + 0.38 * ao)[:, None]

    if part == "head":
        lips = 1.0 - smoothstep(0.55, 1.0, np.sqrt((x / 0.030) ** 2 + ((z - 0.6185) / 0.0165) ** 2))
        lips *= (y < -0.095)
        col = mix(col, srgb_hex(118, 112, 46), 0.85 * lips)
        for sx in (1, -1):                                       # heavy dark brows
            d = np.sqrt(((x - sx * 0.034) / 0.042) ** 2 + ((z - (0.7195 - 0.003 * (1 - abs(x) / 0.075))) / 0.0105) ** 2 + ((y + 0.090) / 0.03) ** 2)
            col = mix(col, srgb_hex(22, 44, 12), 0.9 * (1.0 - smoothstep(0.6, 1.0, d)))
            bag = np.sqrt(((x - sx * 0.036) / 0.026) ** 2 + ((z - 0.676) / 0.010) ** 2 + ((y + 0.082) / 0.03) ** 2)
            col = mix(col, srgb_hex(60, 100, 26), 0.5 * (1.0 - smoothstep(0.5, 1.0, bag)))
        stubble = smoothstep(0.55, 0.8, N.fbm(pos, 420.0, 1)) * smoothstep(0.64, 0.60, z) * (y < 0.0)
        col = mix(col, srgb_hex(52, 92, 24), 0.35 * stubble)
    if part == "hand":
        knuckle = smoothstep(0.6, 0.95, N.fbm(pos, 160.0, 2)) * 0.25
        col = col * (1.0 - knuckle)[:, None]
    spec = np.stack([0.30 + 0.12 * N.fbm(pos, 30.0, 2), 0.55 + 0 * x, 0.15 + 0 * x], axis=1).astype(np.float32)
    spec[:, 0] *= (0.7 + 0.3 * ao)
    if part == "head":
        spec[:, 0] = np.where(y < -0.095, spec[:, 0] * 1.5, spec[:, 0])   # lips shinier
    return col.astype(np.float32), spec


def denim(pos, nrm, ao, N):
    """Faded, filthy, torn-at-the-hem denim."""
    x, y, z = pos[:, 0], pos[:, 1], pos[:, 2]
    deep, faded = srgb_hex(66, 96, 140), srgb_hex(146, 178, 212)
    wear = np.clip(0.42 + (N.fbm(pos, 6.0, 4) - 0.5) * 1.7, 0, 1)
    wear = np.clip(wear + 0.35 * smoothstep(0.80, 1.0, ao) * smoothstep(0.35, 0.65, N.fbm(pos, 9.0, 2)), 0, 1)
    col = mix(deep, faded, wear)
    # woven twill-ish grain
    col = col * (0.90 + 0.20 * N.fbm(pos, 500.0, 1))[:, None]
    col = mix(col, srgb_hex(30, 46, 76), 0.6 * smoothstep(0.70, 0.30, ao))   # folds / seams darker
    col = col * (0.55 + 0.45 * ao)[:, None]
    # grime on the lower legs and around the hem
    grime = smoothstep(-0.30, -0.70, z) * (0.35 + 0.65 * N.fbm(pos, 14.0, 3))
    col = mix(col, srgb_hex(78, 70, 54), 0.55 * grime)
    # torn hem: bleached frayed threads, dark dirty edge
    side = np.where(x >= 0, 1.0, -1.0)
    hem = np.where(side > 0, meshing.hem_height(x, y, 1), meshing.hem_height(x, y, -1))
    dz = z - hem
    fray = smoothstep(0.05, 0.0, dz)
    threads = smoothstep(0.35, 0.8, N.fbm(pos * np.array([40.0, 40.0, 4.0]), 1.0, 2))
    col = mix(col, srgb_hex(206, 212, 206), 0.75 * fray * threads)
    col = mix(col, srgb_hex(52, 46, 36), 0.6 * smoothstep(0.012, 0.0, dz))
    # waistband
    wz = anatomy.WAIST_Z + anatomy.WAIST_TILT * y
    col = mix(col, srgb_hex(40, 62, 100), 0.7 * smoothstep(0.045, 0.02, wz - z))
    spec = np.stack([0.10 + 0 * x, 0.30 + 0 * x, 0.0 * x], axis=1).astype(np.float32)
    return col.astype(np.float32), spec


def hair(pos, nrm, ao, N):
    streak = N.fbm(pos * np.array([1.0, 4.0, 1.0]), 90.0, 2)
    col = mix(srgb_hex(16, 14, 12), srgb_hex(52, 46, 40), smoothstep(0.35, 0.85, streak))
    col = col * (0.5 + 0.5 * ao)[:, None]
    spec = np.stack([0.35 * (0.5 + streak), 0.7 + 0 * ao, 0.0 * ao], axis=1).astype(np.float32)
    return col.astype(np.float32), spec
