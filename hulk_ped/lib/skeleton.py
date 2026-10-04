"""GTA V ped skeleton: loading, rest-pose maths and Sollumz armature creation.

The skeleton data (``data/ped_skeleton.json``) is the stock GTA V ped skeleton (549 bones) as shipped with
Sollumz' ``tools/BoneProperties.xml``.  Bone order, names, tags and hierarchy are kept exactly as in the game,
because exported vertex weights refer to bone *indices*.
"""
import json
import os

import numpy as np

DATA = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "data", "ped_skeleton.json")


def load_bones(path=DATA):
    with open(path) as f:
        return json.load(f)["bones"]


def _quat_to_mat(q):
    w, x, y, z = q
    n = w * w + x * x + y * y + z * z
    s = 2.0 / n if n else 0.0
    return np.array([
        [1 - s * (y * y + z * z), s * (x * y - z * w), s * (x * z + y * w)],
        [s * (x * y + z * w), 1 - s * (x * x + z * z), s * (y * z - x * w)],
        [s * (x * z - y * w), s * (y * z + x * w), 1 - s * (x * x + y * y)],
    ])


def rest_world_matrices(bones):
    """4x4 world matrix of every bone in the rest pose (RAGE bone-local frames)."""
    out = []
    for b in bones:
        m = np.eye(4)
        m[:3, :3] = _quat_to_mat(b["q"]) * np.array(b["s"])[None, :]
        m[:3, 3] = b["t"]
        out.append(out[b["parent"]] @ m if b["parent"] >= 0 else m)
    return out


class Joints:
    """Name -> world position lookup for the rest pose, in metres (origin = pelvis, character faces -Y)."""

    def __init__(self, bones=None):
        self.bones = bones or load_bones()
        self.index = {b["name"]: i for i, b in enumerate(self.bones)}
        self.mats = rest_world_matrices(self.bones)
        self.pos = np.array([m[:3, 3] for m in self.mats])

    def __getitem__(self, name):
        return self.pos[self.index[name]].copy()

    def __contains__(self, name):
        return name in self.index

    def children(self, name):
        i = self.index[name]
        return [b["name"] for b in self.bones if b["parent"] == i]


if __name__ == "__main__":
    import sys

    J = Joints()
    np.set_printoptions(precision=3, suppress=True)
    for n in sys.argv[1:] or ["SKEL_Pelvis", "SKEL_Head"]:
        print(f"{n:28s}", J[n])
