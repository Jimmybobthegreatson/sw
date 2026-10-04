"""Vertex weights against the GTA V ped skeleton.

Every deform bone is reduced to a segment (joint -> next joint).  A vertex gets a Gaussian-falloff weight from each
nearby segment (radius ~ limb thickness), the arm / leg bones are gated so torso vertices near an armpit don't follow
the arm, the weights are lightly smoothed over the mesh, and the best four influences are kept (RAGE limit).
"""
import numpy as np
from scipy.sparse import coo_matrix, diags

from .bake import smoothstep


def _seg(a, b):
    return np.asarray(a, np.float64), np.asarray(b, np.float64)


def build_segments(J):
    """name -> (a, b, radius).  Left (+X) and right (-X) sides are generated from the same description."""
    S = {}
    g = J.__getitem__

    def add(name, a, b, r):
        S[name] = (np.asarray(a, np.float64), np.asarray(b, np.float64), r)

    mid_hip = (g("SKEL_L_Thigh") + g("SKEL_R_Thigh")) / 2
    add("SKEL_Pelvis", g("SKEL_Pelvis"), mid_hip + np.array([0, 0, -0.06]), 0.20)
    add("SKEL_Spine_Root", g("SKEL_Spine_Root"), g("SKEL_Spine0"), 0.16)
    add("SKEL_Spine0", g("SKEL_Spine0"), g("SKEL_Spine1"), 0.18)
    add("SKEL_Spine1", g("SKEL_Spine1"), g("SKEL_Spine2"), 0.20)
    add("SKEL_Spine2", g("SKEL_Spine2"), g("SKEL_Spine3"), 0.22)
    add("SKEL_Spine3", g("SKEL_Spine3"), g("SKEL_Neck_1") + np.array([0, 0, -0.03]), 0.26)
    add("SKEL_Neck_1", g("SKEL_Neck_1"), g("SKEL_Head"), 0.085)
    add("SKEL_Head", g("SKEL_Head"), g("SKEL_Head") + np.array([0, -0.02, 0.14]), 0.12)
    for side, sx in (("L", 1), ("R", -1)):
        n = lambda s: f"SKEL_{side}_{s}"
        add(n("Clavicle"), g(n("Clavicle")), g(n("UpperArm")), 0.075)
        add(n("UpperArm"), g(n("UpperArm")), g(n("Forearm")), 0.105)
        add(n("Forearm"), g(n("Forearm")), g(n("Hand")), 0.085)
        knuckle = np.mean([g(n(f"Finger{i}0")) for i in range(1, 5)], axis=0)
        add(n("Hand"), g(n("Hand")), knuckle, 0.065)
        for i in range(5):
            a, b, c = g(n(f"Finger{i}0")), g(n(f"Finger{i}1")), g(n(f"Finger{i}2"))
            tip = c + (c - b) / np.linalg.norm(c - b) * 0.026
            r = 0.019 if i == 0 else 0.016
            add(n(f"Finger{i}0"), a, b, r)
            add(n(f"Finger{i}1"), b, c, r * 0.95)
            add(n(f"Finger{i}2"), c, tip, r * 0.9)
        add(n("Thigh"), g(n("Thigh")), g(n("Calf")), 0.17)
        add(n("Calf"), g(n("Calf")), g(n("Foot")), 0.11)
        add(n("Foot"), g(n("Foot")), g(n("Toe0")), 0.085)
        toe = g(n("Toe0"))
        add(n("Toe0"), toe, toe + np.array([0, -0.075, -0.004]), 0.035)
    return S


def _dist(P, a, b):
    ab = b - a
    t = np.clip(((P - a) @ ab) / max(float(ab @ ab), 1e-12), 0.0, 1.0)
    return np.linalg.norm(P - (a + t[:, None] * ab), axis=1), t


def compute_weights(V, F, J, kind="body", smooth_iters=2):
    """Return (names list, weights array (N, 4) float32, bone index array (N, 4) int) for mesh ``V``/``F``.

    ``kind``: 'body' (skin, jeans), 'head' (adds jaw / eyeball influence), 'hair' (rigid to the head).
    """
    S = build_segments(J)
    names = list(S)
    P = V.astype(np.float64)
    n = len(P)
    W = np.zeros((n, len(names)))
    t_along = {}
    for k, name in enumerate(names):
        a, b, r = S[name]
        d, t = _dist(P, a, b)
        W[:, k] = np.exp(-((d / r) ** 2) * 1.6) * (d < 2.4 * r)
        t_along[name] = t

    idx = {nm: i for i, nm in enumerate(names)}
    ax = np.abs(P[:, 0])
    side = np.where(P[:, 0] >= 0, "L", "R")

    # gates: arm bones only act beyond the shoulder, leg bones only below the hip, spine only on the torso
    for s in ("L", "R"):
        m = side == s
        sh = J[f"SKEL_{s}_UpperArm"]
        arm_dir = (J[f"SKEL_{s}_Forearm"] - sh)
        arm_dir = arm_dir / np.linalg.norm(arm_dir)
        beyond = (P - sh) @ arm_dir  # metres past the shoulder joint along the arm
        gate_arm = smoothstep(-0.03, 0.05, beyond)
        for nm in [x for x in names if x.startswith(f"SKEL_{s}_") and any(
                q in x for q in ("UpperArm", "Forearm", "Hand", "Finger"))]:
            W[:, idx[nm]] *= np.where(m, gate_arm, 0.0)
        hip = J[f"SKEL_{s}_Thigh"]
        gate_leg = smoothstep(hip[2] + 0.03, hip[2] - 0.07, P[:, 2])
        for nm in [x for x in names if x.startswith(f"SKEL_{s}_") and any(q in x for q in ("Thigh", "Calf", "Foot", "Toe"))]:
            W[:, idx[nm]] *= np.where(m, gate_leg, 0.0)
        W[:, idx[f"SKEL_{s}_Clavicle"]] *= np.where(m, 1.0, 0.0)
    torso_gate = smoothstep(-0.30, -0.10, P[:, 2])
    for nm in ("SKEL_Spine_Root", "SKEL_Spine0", "SKEL_Spine1", "SKEL_Spine2", "SKEL_Spine3"):
        W[:, idx[nm]] *= torso_gate
    # head bone only above the neck, neck bone in the neck
    W[:, idx["SKEL_Head"]] *= smoothstep(0.545, 0.60, P[:, 2])
    W[:, idx["SKEL_Neck_1"]] *= smoothstep(0.46, 0.50, P[:, 2]) * (1.0 - 0.7 * smoothstep(0.63, 0.70, P[:, 2]))
    # fingers: a vertex belongs to the nearest finger only
    for s in ("L", "R"):
        fing = [idx[f"SKEL_{s}_Finger{i}{j}"] for i in range(5) for j in range(3)]
        W[:, fing] = W[:, fing] ** 1.5

    if kind == "hair":
        W[:] = 0.0
        W[:, idx["SKEL_Head"]] = 1.0

    # sharpen toward the dominant influence
    W = W ** 1.25
    tot = W.sum(1, keepdims=True)
    # vertices that no segment reaches: use the nearest segment
    far = tot[:, 0] < 1e-6
    if far.any():
        dd = np.stack([_dist(P[far], *S[nm][:2])[0] / S[nm][2] for nm in names], axis=1)
        W[far, dd.argmin(1)] = 1.0
        tot = W.sum(1, keepdims=True)
    W /= tot

    extra = []
    if kind == "head":
        # jaw follows speech; eyeballs follow the eye bones
        extra_names = ["FACIAL_jaw"]
        jaw = smoothstep(0.628, 0.598, P[:, 2]) * smoothstep(0.045, -0.02, P[:, 1]) * (ax < 0.085)
        extra = [(extra_names[0], jaw * 0.85)]

    # smooth over the surface
    if smooth_iters and kind != "hair":
        e = np.concatenate([F[:, [0, 1]], F[:, [1, 2]], F[:, [2, 0]]])
        A = coo_matrix((np.ones(len(e)), (e[:, 0], e[:, 1])), shape=(n, n)).tocsr()
        A = ((A + A.T) > 0).astype(np.float64)
        L = diags(1.0 / np.maximum(np.asarray(A.sum(1)).ravel(), 1.0)) @ A
        for _ in range(smooth_iters):
            W = 0.5 * W + 0.5 * (L @ W)
        W /= np.maximum(W.sum(1, keepdims=True), 1e-9)

    all_names = names + [nm for nm, _ in extra]
    if extra:
        Wf = np.concatenate([W, np.zeros((n, len(extra)))], axis=1)
        for k, (nm, wv) in enumerate(extra):
            Wf[:, len(names) + k] = wv
            Wf[:, :len(names)] *= (1.0 - wv)[:, None]
        W = Wf

    top = np.argsort(-W, axis=1)[:, :4]
    wt = np.take_along_axis(W, top, axis=1)
    wt[wt < 0.015] = 0.0
    wt /= np.maximum(wt.sum(1, keepdims=True), 1e-9)
    return all_names, wt.astype(np.float32), top.astype(np.int32)
