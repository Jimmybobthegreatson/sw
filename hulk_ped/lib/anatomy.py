"""Hulk anatomy as signed distance fields, sized from the GTA V ped skeleton's rest pose.

Coordinates: metres, origin at the pelvis, character faces -Y, his left side is +X (like the game's ped skeleton),
feet on z = -1.0.  Every muscle mass is placed relative to the real joint positions so the mesh fits the rig.
"""
import numpy as np

from .sdf import Grid, align_z, euler, rot_x, rot_y, rot_z

GROUND_Z = -1.0
NECK_CUT_Z = 0.548          # skin faces above this height (and narrower than NECK_CUT_X) belong to the head part
NECK_CUT_X = 0.100
SKIN_UPPR_BOTTOM = -0.20    # the torso skin ends inside the jeans
GRID_LO = (-0.80, -0.30, -1.03)
GRID_HI = (0.80, 0.32, 0.86)


def lerp(a, b, t):
    return np.asarray(a, np.float64) * (1 - t) + np.asarray(b, np.float64) * t


def unit(v):
    v = np.asarray(v, np.float64)
    return v / np.linalg.norm(v)


class Rig:
    """Handy named joint positions for the left side (+X) and the spine."""

    def __init__(self, J):
        g = J.__getitem__
        self.hip, self.knee, self.ankle, self.toe = g("SKEL_L_Thigh"), g("SKEL_L_Calf"), g("SKEL_L_Foot"), g("SKEL_L_Toe0")
        self.clav, self.shoulder, self.elbow, self.wrist = g("SKEL_L_Clavicle"), g("SKEL_L_UpperArm"), g("SKEL_L_Forearm"), g("SKEL_L_Hand")
        self.neck, self.head = g("SKEL_Neck_1"), g("SKEL_Head")
        self.fingers = [[g(f"SKEL_L_Finger{i}{j}") for j in range(3)] for i in range(5)]  # i: 0 thumb .. 4 pinky
        self.eye = g("FACIAL_L_eyeball")
        self.spine = [g(n) for n in ("SKEL_Spine_Root", "SKEL_Spine0", "SKEL_Spine1", "SKEL_Spine2", "SKEL_Spine3")]


# --------------------------------------------------------------------------------------------------------------------
# head
# --------------------------------------------------------------------------------------------------------------------
def build_head(g: Grid, R: Rig):
    """Skull, square jaw, brow, nose, lips, ears and the neck stump (anchored on the rig's facial joints)."""
    # cranium + mid-face
    g.ellipsoid((0, -0.002, 0.697), (0.080, 0.100, 0.083))
    g.ellipsoid((0, -0.042, 0.652), (0.073, 0.072, 0.070), k=0.02)
    # heavy, square jaw and a wide flat chin
    g.ellipsoid((0, -0.028, 0.607), (0.073, 0.080, 0.046), k=0.025)
    g.ellipsoid((0.062, 0.012, 0.620), (0.022, 0.046, 0.040), k=0.02, sym=True)                    # jaw angle / masseter
    g.ellipsoid((0.024, -0.086, 0.591), (0.032, 0.030, 0.028), k=0.015, sym=True)                  # chin
    g.ellipsoid((0.0, -0.094, 0.586), (0.030, 0.022, 0.022), k=0.010)
    # cheekbones
    g.ellipsoid((0.050, -0.074, 0.666), (0.027, 0.024, 0.017), rot=euler(0, 0, -20), k=0.014, sym=True)
    # brow ridge: wraps the forehead, inner ends dip -> permanent scowl
    g.ellipsoid((0.032, -0.087, 0.719), (0.034, 0.019, 0.012), rot=euler(0, -13, 20), k=0.014, sym=True)
    g.ellipsoid((0.0, -0.093, 0.713), (0.018, 0.016, 0.013), k=0.010)                              # glabella

    # eye sockets: carve, lids half-close the eyeball (eyeballs are separate meshes)
    ex, ey, ez = R.eye
    g.ellipsoid((ex, ey - 0.016, ez), (0.0215, 0.0225, 0.0195), k=0.006, op="sub", sym=True)
    g.ellipsoid((ex, ey - 0.002, ez + 0.0150), (0.0225, 0.0190, 0.0085), rot=euler(0, -15, 0), k=0.004, sym=True)   # upper lid
    g.ellipsoid((ex, ey - 0.003, ez - 0.0152), (0.0205, 0.0170, 0.0070), rot=euler(0, 5, 0), k=0.004, sym=True)      # lower lid
    g.ellipsoid((ex * 1.05, ey + 0.012, ez - 0.026), (0.020, 0.012, 0.010), k=0.012, sym=True)                      # eye bag

    # nose: broad and flattened
    g.capsule((0, -0.098, 0.710), (0, -0.116, 0.672), 0.0090, 0.0135, k=0.010)                 # bridge
    g.ellipsoid((0, -0.121, 0.665), (0.0170, 0.0150, 0.0140), k=0.010)                         # tip
    g.ellipsoid((0.0200, -0.108, 0.661), (0.0130, 0.0135, 0.0115), rot=euler(0, 0, 22), k=0.010, sym=True)  # ala
    g.ellipsoid((0.0110, -0.116, 0.652), (0.0058, 0.0070, 0.0048), k=0.003, op="sub", sym=True)             # nostrils

    # lips: a mouth block holds both lips together (the mouth line itself is a crease in the detail pass)
    g.ellipsoid((0, -0.106, 0.6185), (0.030, 0.017, 0.0175), k=0.008)
    g.ellipsoid((0, -0.109, 0.6275), (0.026, 0.014, 0.0090), k=0.006)                          # upper lip
    g.ellipsoid((0, -0.109, 0.6100), (0.025, 0.014, 0.0100), k=0.006)                          # lower lip
    g.ellipsoid((0.0235, -0.100, 0.6160), (0.0100, 0.012, 0.0110), k=0.006, sym=True)          # mouth corners

    # ears
    g.ellipsoid((0.079, 0.006, 0.678), (0.0085, 0.020, 0.031), rot=euler(0, 0, 8), k=0.008, sym=True)
    g.ellipsoid((0.078, 0.004, 0.654), (0.0075, 0.010, 0.010), k=0.004, sym=True)              # lobe

    # neck
    g.tube([(0, 0.030, 0.500), (0, 0.020, 0.560), (0, 0.004, 0.625)], [0.098, 0.086, 0.072], [0.090, 0.080, 0.068],
           k=0.02, up=(0, 1, 0))
    g.capsule((0.030, -0.030, 0.605), (0.060, -0.020, 0.545), 0.018, 0.026, k=0.014, sym=True)  # sternocleidomastoid
    g.ellipsoid((0, -0.075, 0.566), (0.016, 0.018, 0.016), k=0.01)                              # adam's apple


# --------------------------------------------------------------------------------------------------------------------
# torso
# --------------------------------------------------------------------------------------------------------------------
def build_torso(g: Grid, R: Rig):
    # main trunk: V-taper from wide shoulders to a still-thick waist
    g.loft([
        (-0.17, 0.000, 0.004, 0.215, 0.150),
        (-0.08, 0.000, 0.004, 0.240, 0.155),
        (0.02, 0.000, -0.004, 0.205, 0.140),
        (0.12, 0.000, -0.010, 0.222, 0.146),
        (0.22, 0.000, -0.014, 0.256, 0.156),
        (0.31, 0.000, -0.018, 0.288, 0.166),
        (0.40, 0.000, -0.020, 0.292, 0.168),
        (0.47, 0.000, -0.008, 0.236, 0.140),
        (0.53, 0.000, 0.012, 0.140, 0.105),
    ], power=2.3)

    # pectorals: broad plates that slope into the front deltoid
    g.ellipsoid((0.108, -0.130, 0.388), (0.124, 0.062, 0.082), rot=euler(-6, 0, -16), k=0.03, sym=True)
    g.ellipsoid((0.185, -0.085, 0.425), (0.075, 0.062, 0.070), k=0.03, sym=True)
    # abdominal plate
    g.box((0, -0.140, 0.120), (0.078, 0.040, 0.190), rounding=0.034, k=0.05)
    # obliques / serratus
    g.ellipsoid((0.190, -0.060, 0.17), (0.058, 0.092, 0.125), rot=euler(0, 0, -8), k=0.03, sym=True)
    for i, z in enumerate((0.31, 0.26, 0.21)):
        g.ellipsoid((0.230 - 0.004 * i, -0.082, z), (0.024, 0.030, 0.011), rot=euler(0, 0, 25), k=0.012, sym=True)
    # latissimus: wide flare that wraps from the back to the sides
    g.ellipsoid((0.210, 0.095, 0.275), (0.080, 0.050, 0.150), rot=euler(0, 0, -14), k=0.04, sym=True)
    # back: erectors, mid traps
    g.ellipsoid((0.045, 0.118, 0.16), (0.034, 0.034, 0.12), k=0.025, sym=True)
    g.ellipsoid((0.07, 0.115, 0.40), (0.075, 0.040, 0.070), k=0.03, sym=True)
    # trapezius: heavy slope from skull to shoulder
    g.capsule((0.040, 0.026, 0.575), (0.215, 0.040, 0.495), 0.060, 0.058, k=0.03, sym=True)
    g.ellipsoid((0.0, 0.066, 0.490), (0.170, 0.058, 0.105), k=0.03)
    g.ellipsoid((0.110, 0.030, 0.530), (0.090, 0.065, 0.050), rot=euler(0, 0, -22), k=0.035, sym=True)
    # clavicles
    g.capsule((0.020, -0.080, 0.505), (0.165, -0.050, 0.492), 0.012, 0.013, k=0.008, sym=True)
    # glutes / pelvis (hidden by jeans but part of the base body)
    g.ellipsoid((0.092, 0.075, -0.125), (0.105, 0.095, 0.105), k=0.03, sym=True)
    g.ellipsoid((0, -0.05, -0.15), (0.14, 0.10, 0.08), k=0.03)


# --------------------------------------------------------------------------------------------------------------------
# arms
# --------------------------------------------------------------------------------------------------------------------
def build_arm(g: Grid, R: Rig):
    S, E, W = R.shoulder, R.elbow, R.wrist
    upper = unit(E - S)
    fore = unit(W - E)
    Ru = align_z(upper)
    Rf = align_z(fore)

    # deltoid cap (anterior / lateral / posterior heads)
    g.ellipsoid(S + np.array([0.030, 0.000, 0.000]), (0.112, 0.100, 0.118), k=0.02, sym=True)
    g.ellipsoid(S + np.array([0.040, -0.050, -0.015]), (0.066, 0.058, 0.090), rot=euler(0, 0, -20), k=0.03, sym=True)
    g.ellipsoid(S + np.array([0.040, 0.050, -0.015]), (0.066, 0.058, 0.090), rot=euler(0, 0, -20), k=0.03, sym=True)
    # upper arm
    g.tube([S, E], [0.100, 0.080], [0.094, 0.078], k=0.03, sym=True)
    g.ellipsoid(lerp(S, E, 0.42) + np.array([0.0, -0.040, 0.0]), (0.056, 0.058, 0.105), rot=Ru, k=0.035, sym=True)   # biceps
    g.ellipsoid(lerp(S, E, 0.40) + np.array([0.0, 0.042, 0.0]), (0.056, 0.056, 0.110), rot=Ru, k=0.035, sym=True)    # triceps
    g.ellipsoid(lerp(S, E, 0.45) + np.array([0.0, 0.0, 0.0]), (0.050, 0.020, 0.100), rot=Ru, k=0.005, op="sub", sym=True)  # bicep/tricep split
    # elbow + forearm
    g.ellipsoid(E, (0.072, 0.070, 0.070), k=0.02, sym=True)
    g.tube([E, lerp(E, W, 0.35), W], [0.082, 0.080, 0.046], [0.078, 0.080, 0.050], k=0.02, sym=True)
    g.ellipsoid(lerp(E, W, 0.28) + np.array([0.0, -0.012, 0.0]), (0.070, 0.070, 0.125), rot=Rf, k=0.04, sym=True)      # brachioradialis / flexors
    g.ellipsoid(lerp(E, W, 0.32) + np.array([0.0, 0.028, 0.0]), (0.060, 0.050, 0.110), rot=Rf, k=0.04, sym=True)      # extensors
    g.ellipsoid(W, (0.047, 0.050, 0.050), k=0.015, sym=True)


def build_hand(g: Grid, R: Rig):
    W = R.wrist
    F = [[p.copy() for p in f] for f in R.fingers]
    # spread the fingers a little so they stay readable as separate fingers (max ~1 cm off their bones)
    ymid = np.mean([F[i][0][1] for i in range(1, 5)])
    for i in range(1, 5):
        for j in range(3):
            F[i][j][1] += (F[i][0][1] - ymid) * 0.30 * (0.6 + 0.2 * j)
    knuckles = np.mean([F[i][0] for i in range(1, 5)], axis=0)
    axis = knuckles - W
    Rh = align_z(axis, up=(0, 1, 0))
    # palm, thenar (thumb) mound, back of hand and knuckle bar
    g.ellipsoid(lerp(W, knuckles, 0.50), (0.034, 0.064, 0.060), rot=Rh, k=0.02, sym=True)
    g.ellipsoid(lerp(W, F[0][1], 0.55) + np.array([-0.004, -0.012, 0.0]), (0.032, 0.036, 0.048), rot=Rh, k=0.015, sym=True)
    g.ellipsoid(lerp(W, knuckles, 0.97), (0.028, 0.060, 0.022), rot=Rh, k=0.012, sym=True)
    # fingers: base joint -> mid -> distal -> tip
    radii = {1: 0.0150, 2: 0.0158, 3: 0.0148, 4: 0.0132}
    for i in range(1, 5):
        j0, j1, j2 = F[i]
        d = unit(j2 - j1)
        tip = j2 + d * 0.028
        r = radii[i]
        g.capsule(j0, j1, r, r * 0.94, k=0.004, sym=True)
        g.capsule(j1, j2, r * 0.94, r * 0.88, k=0.004, sym=True)
        g.capsule(j2, tip, r * 0.88, r * 0.72, k=0.004, sym=True)
        g.ellipsoid(j1, (r * 1.10, r * 1.08, r * 1.12), k=0.003, sym=True)           # knuckle
    # thumb
    j0, j1, j2 = F[0]
    d = unit(j2 - j1)
    tip = j2 + d * 0.027
    g.capsule(j0, j1, 0.0205, 0.0180, k=0.008, sym=True)
    g.capsule(j1, j2, 0.0180, 0.0165, k=0.005, sym=True)
    g.capsule(j2, tip, 0.0165, 0.0135, k=0.005, sym=True)
    # light finger separation grooves
    for i in range(1, 4):
        a_ = (F[i][0] + F[i + 1][0]) / 2
        b_ = (F[i][1] + F[i + 1][1]) / 2
        c_ = (F[i][2] + F[i + 1][2]) / 2 + np.array([0, 0, -0.016])
        g.capsule(lerp(a_, b_, 0.35), b_, 0.0016, 0.0016, k=0.0015, op="sub", sym=True)
        g.capsule(b_, c_, 0.0016, 0.0016, k=0.0015, op="sub", sym=True)


# --------------------------------------------------------------------------------------------------------------------
# legs
# --------------------------------------------------------------------------------------------------------------------
def build_leg_core(g: Grid, R: Rig, inflate=0.0, flare=0.0):
    """Thigh, knee and calf; ``inflate`` grows them (used for the baggy jeans), ``flare`` widens towards the hem."""
    H, K, A = R.hip, R.knee, R.ankle
    Rt = align_z(unit(K - H))
    Rc = align_z(unit(A - K))
    i = inflate
    M = lerp(H, K, 0.5)
    C = lerp(K, A, 0.30)
    g.tube([H + np.array([0.0, 0.0, 0.04]), M, K, C, A],
           [0.150 + i, 0.124 + i, 0.096 + i, 0.086 + i + flare * 0.3, 0.052 + i + flare],
           [0.150 + i, 0.126 + i, 0.102 + i, 0.092 + i + flare * 0.3, 0.056 + i + flare], k=0.03, sym=True)
    g.ellipsoid(lerp(H, K, 0.42) + np.array([0.0, -0.040, 0.0]), (0.100 + i, 0.092 + i, 0.215), rot=Rt, k=0.06, sym=True)     # quads
    g.ellipsoid(lerp(H, K, 0.42) + np.array([0.0, 0.048, 0.0]), (0.098 + i, 0.080 + i, 0.215), rot=Rt, k=0.06, sym=True)     # hamstrings
    g.ellipsoid(lerp(K, A, 0.27) + np.array([0.012, 0.038, 0.0]), (0.058 + i, 0.062 + i, 0.150), rot=Rc, k=0.05, sym=True)   # gastrocnemius
    g.ellipsoid(lerp(K, A, 0.30) + np.array([-0.028, 0.032, 0.0]), (0.048 + i, 0.056 + i, 0.140), rot=Rc, k=0.05, sym=True)  # inner calf


def build_foot(g: Grid, R: Rig):
    A, T = R.ankle, R.toe
    gz = GROUND_Z
    g.ellipsoid(A + np.array([0.0, 0.0, -0.010]), (0.050, 0.052, 0.055), k=0.02, sym=True)                       # ankle
    g.ellipsoid((A[0] + 0.001, A[1] + 0.030, gz + 0.040), (0.046, 0.054, 0.042), k=0.02, sym=True)               # heel
    g.tube([(A[0], A[1] + 0.02, gz + 0.050), (A[0] + 0.004, T[1] + 0.045, gz + 0.040), (T[0] + 0.004, T[1] - 0.015, gz + 0.028)],
           [0.046, 0.054, 0.062], [0.044, 0.036, 0.028], k=0.02, sym=True, up=(0, 0, 1))
    # toes (big toe medial = smaller x)
    tx = [A[0] - 0.030, A[0] - 0.003, A[0] + 0.022, A[0] + 0.043, A[0] + 0.060]
    ln = [0.060, 0.058, 0.050, 0.044, 0.036]
    rr = [0.021, 0.0145, 0.0135, 0.0125, 0.0115]
    for i in range(5):
        y0 = T[1] - 0.005 + 0.004 * i
        g.capsule((tx[i], y0, gz + 0.026), (tx[i] + 0.002 * i, y0 - ln[i], gz + 0.020), rr[i], rr[i] * 0.88, k=0.004, sym=True)
    g.ellipsoid((A[0] - 0.015, T[1] + 0.02, gz + 0.032), (0.040, 0.050, 0.030), k=0.02, sym=True)               # ball of foot
    g.halfspace((0, 0, gz), (0, 0, -1), k=0.004, op="intersect")


# --------------------------------------------------------------------------------------------------------------------
# --------------------------------------------------------------------------------------------------------------------
# jeans (a baggy surface around pelvis + legs; the waist and hems are cut open when the mesh is split into parts)
# --------------------------------------------------------------------------------------------------------------------
WAIST_Z = -0.030          # waistband height at the sides (higher at the back)
WAIST_TILT = 0.20         # rise of the waistband per metre towards the back
HEM_Z = (-0.700, -0.760)  # torn hem height: left leg (+X), right leg (-X)


def build_jeans(g: Grid, R: Rig):
    g.loft([
        (-0.34, 0.000, 0.004, 0.196, 0.148),
        (-0.22, 0.000, 0.006, 0.250, 0.172),
        (-0.10, 0.000, 0.006, 0.258, 0.176),
        (0.02, 0.000, 0.000, 0.238, 0.164),
    ], power=2.4)
    build_leg_core(g, R, inflate=0.026, flare=0.020)
    g.ellipsoid((0.0, 0.0, -0.250), (0.130, 0.130, 0.140), k=0.05)           # crotch fill
    g.ellipsoid((0.0, 0.085, -0.150), (0.20, 0.10, 0.12), k=0.05)            # seat


def jeans_details(g: Grid, R: Rig):
    c = g.crease_snapped
    ring = g.crease_ring
    zc = (0.0, 0.003)
    # waistband (lower edge) and belt-loop line
    ring(zc, WAIST_Z - 0.050, 0.30, 0.22, 0.0070, 0.0050, tilt=WAIST_TILT)
    ring(zc, WAIST_Z - 0.040, 0.30, 0.22, 0.0030, 0.0030, tilt=WAIST_TILT)
    # fly, front pockets, back yoke and pockets
    c([(0.0, 0, -0.07), (0.0, 0, -0.17)], FRONT, 0.005, 0.0040)
    c([(0.010, 0, -0.17), (0.035, 0, -0.145), (0.037, 0, -0.07)], FRONT, 0.0045, 0.0035)
    c([(0.19, 0, -0.075), (0.15, 0, -0.115), (0.120, 0, -0.17), (0.115, 0, -0.205)], FRONT, 0.0070, 0.0050, sym=True)
    c([(0.04, 0, -0.085), (0.19, 0, -0.14)], BACK, 0.0055, 0.0040, sym=True)
    c([(0.05, 0, -0.14), (0.15, 0, -0.14), (0.14, 0, -0.23), (0.10, 0, -0.255), (0.06, 0, -0.23), (0.05, 0, -0.14)], BACK, 0.0055, 0.0040, sym=True)
    # side and inseam
    H, K, A = R.hip, R.knee, R.ankle
    c([(0.268, 0, -0.07), (0.27, 0, -0.25), (K[0] + 0.115, 0, K[2]), (A[0] + 0.07, 0, A[2] + 0.1)], (-1.0, 0.0, 0.0), 0.0045, 0.0045, sym=True)
    # stacked folds at the knee and calf
    for z0, amp in ((-0.40, 0.0060), (-0.47, 0.0070), (-0.55, 0.0050), (-0.60, 0.0060)):
        ring((K[0], 0.03), z0, 0.16, 0.16, amp, 0.0070, a0=-120, a1=120)      # outer side of the left leg
        ring((-K[0], 0.03), z0, 0.16, 0.16, amp, 0.0070, a0=60, a1=300)       # outer side of the right leg


# --------------------------------------------------------------------------------------------------------------------
# hair: a shell around the skull + swept-back locks; everything under the hairline is cut off at mesh time
# --------------------------------------------------------------------------------------------------------------------
def build_hair(g: Grid, R: Rig):
    rng = np.random.default_rng(7)
    g.ellipsoid((0.0, 0.004, 0.703), (0.0885, 0.1085, 0.0925))
    g.ellipsoid((0.0, -0.060, 0.738), (0.065, 0.060, 0.045), k=0.02)                    # front mass above the brow
    # swept-back locks
    for i in range(46):
        th = rng.uniform(-1.25, 1.25)                       # angle around the head, 0 = straight ahead (-Y)
        el = rng.uniform(0.15, 1.15)                        # elevation, 0 = at the ears, 1.2 = crown
        r0 = np.array([0.0, 0.0, 0.70])
        base = r0 + np.array([np.sin(th) * np.cos(el) * 0.092, -np.cos(th) * np.cos(el) * 0.110, np.sin(el) * 0.092])
        sweep = np.array([-np.sin(th) * 0.3, 1.0, 0.25 * np.cos(el)])
        sweep = sweep / np.linalg.norm(sweep)
        L = rng.uniform(0.030, 0.052)
        tip = base + sweep * L + np.array([0.0, 0.0, 0.006])
        g.capsule(base, tip, rng.uniform(0.011, 0.015), rng.uniform(0.004, 0.007), k=0.008)
    # a few forward tufts
    for x in (-0.035, -0.012, 0.014, 0.038):
        g.capsule((x, -0.080, 0.756), (x * 1.3, -0.098, 0.768), 0.011, 0.005, k=0.008)


def hairline_z(x, y):
    """Height of the hairline surface; hair exists above it."""
    x = np.abs(x)
    return 0.744 - 0.060 * np.clip(x / 0.082, 0, 1) ** 2 - 0.085 * np.clip((y + 0.005) / 0.10, 0, 1)


FRONT = (0.0, 1.0, 0.0)   # snap direction for creases on front-facing skin (ray travels towards +Y)
BACK = (0.0, -1.0, 0.0)


def detail_pass(g: Grid, R: Rig):
    """Grooves between muscle groups, facial lines.  Run after all volumes exist (curves snap onto the surface)."""
    c = g.crease_snapped
    # chest / abdomen
    c([(0, 0, 0.468), (0, 0, 0.332)], FRONT, 0.007, 0.0055)                                              # sternum
    c([(0.018, 0, 0.314), (0.07, 0, 0.306), (0.12, 0, 0.308), (0.18, 0, 0.324), (0.226, 0, 0.352)], FRONT, 0.0075, 0.0085, sym=True)
    c([(0.17, 0, 0.472), (0.215, 0, 0.425), (0.238, 0, 0.372)], FRONT, 0.006, 0.0075, sym=True)         # pec / delt
    c([(0, 0, 0.300), (0, 0, 0.020)], FRONT, 0.0055, 0.0035)                                            # linea alba
    for z in (0.228, 0.158, 0.088):
        c([(0.004, 0, z), (0.082, 0, z - 0.004)], FRONT, 0.0055, 0.0035, sym=True)                      # ab cross-grooves
    c([(0.090, 0, 0.286), (0.095, 0, 0.20), (0.092, 0, 0.10), (0.086, 0, 0.03)], FRONT, 0.0055, 0.0050, sym=True)
    for i, z in enumerate((0.31, 0.26, 0.21)):                                                          # serratus fingers
        c([(0.200 + 0.004 * i, 0, z + 0.016), (0.238, 0, z - 0.006)], FRONT, 0.004, 0.0045, sym=True)
    # back
    c([(0, 0, 0.52), (0, 0, 0.0)], BACK, 0.007, 0.0060)                                                  # spine channel
    c([(0.045, 0, 0.46), (0.062, 0, 0.395), (0.052, 0, 0.33)], BACK, 0.005, 0.0060, sym=True)            # scapula edge
    c([(0.20, 0, 0.42), (0.255, 0, 0.30), (0.235, 0, 0.15)], BACK, 0.006, 0.0100, sym=True)              # lat border
    c([(0.05, 0, 0.55), (0.12, 0, 0.505), (0.20, 0, 0.465)], BACK, 0.006, 0.0080, sym=True)              # trap edge
    c([(0.035, 0, 0.30), (0.04, 0, 0.10)], BACK, 0.004, 0.0050, sym=True)                                # erectors
    # face
    c([(-0.021, 0, 0.6195), (0, 0, 0.6190), (0.021, 0, 0.6195), (0.034, 0, 0.6125)], FRONT, 0.0040, 0.0022)
    c([(-0.021, 0, 0.6195), (-0.034, 0, 0.6125)], FRONT, 0.0040, 0.0022)
    c([(0.026, 0, 0.652), (0.036, 0, 0.634), (0.043, 0, 0.616)], FRONT, 0.004, 0.0045, sym=True)         # nasolabial fold
    c([(-0.020, 0, 0.5955), (0.020, 0, 0.5955)], FRONT, 0.004, 0.0040)                                   # chin-lip dimple
    c([(0.005, 0, 0.728), (0.005, 0, 0.750)], FRONT, 0.0035, 0.0040, sym=True)                            # brow furrow
    # arms
    S, E = R.shoulder, R.elbow
    c([(S[0] + 0.02, S[1], S[2] - 0.05), (lerp(S, E, 0.5)[0] + 0.03, 0, lerp(S, E, 0.5)[2] + 0.03), (E[0] + 0.01, 0, E[2] + 0.07)],
      FRONT, 0.005, 0.0070, sym=True)                                                                  # biceps outline (front)


def build_skin(g: Grid, R: Rig):
    build_torso(g, R)
    build_head(g, R)
    build_arm(g, R)
    build_hand(g, R)
    build_leg_core(g, R)
    build_foot(g, R)
    detail_pass(g, R)


def build_jeans_full(g: Grid, R: Rig):
    build_jeans(g, R)
    jeans_details(g, R)


def build_hair_full(g: Grid, R: Rig):
    build_hair(g, R)


def new_grid(h):
    return Grid(GRID_LO, GRID_HI, h)
