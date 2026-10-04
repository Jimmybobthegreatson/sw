"""Create the GTA V ped armature through Sollumz and attach weighted meshes to it."""
import numpy as np

from .skeleton import load_bones


def make_skeleton():
    from mathutils import Quaternion, Vector
    from szio.gta5 import SkelBone, SkelBoneFlags, Skeleton
    from szio.gta5.cwxml.adapters.drawable import CW_BONE_FLAGS_MAP

    bones = []
    for b in load_bones():
        flags = SkelBoneFlags(0)
        for f in b["flags"]:
            flags |= CW_BONE_FLAGS_MAP[f]
        bones.append(SkelBone(name=b["name"], tag=b["tag"], flags=flags, position=Vector(b["t"]),
                              rotation=Quaternion(b["q"]), scale=Vector(b["s"]), parent_index=b["parent"],
                              translation_limit=None, rotation_limit=None))
    return Skeleton(bones)


def create_armature(name):
    """Armature object of Sollumz type *Drawable Dictionary* holding the full 549-bone ped skeleton."""
    from Sollumz.ydr.ydrimport import create_armature_obj_from_skel
    from Sollumz.sollumz_properties import SollumType

    return create_armature_obj_from_skel(make_skeleton(), name, SollumType.DRAWABLE_DICTIONARY)


def share_armature(name, source_obj):
    """Another Drawable Dictionary object that reuses ``source_obj``'s armature data (one skeleton, many files)."""
    import bpy
    from Sollumz.sollumz_properties import SollumType
    from Sollumz.tools.blenderhelper import create_blender_object

    return create_blender_object(SollumType.DRAWABLE_DICTIONARY, name, source_obj.data)


def assign_weights(obj, bone_names, weights, bone_idx):
    """Vertex groups named after bones.  ``weights``/``bone_idx``: (N, 4) arrays; ``bone_names``: columns' names."""
    groups = {}
    n = len(weights)
    used = np.unique(bone_idx[weights > 0])
    for u in used:
        groups[int(u)] = obj.vertex_groups.new(name=bone_names[int(u)])
    for k in range(4):
        for u in np.unique(bone_idx[:, k]):
            sel = np.where((bone_idx[:, k] == u) & (weights[:, k] > 0))[0]
            if len(sel) == 0:
                continue
            # vertices may carry the same bone twice only if a weight was zeroed; add accumulates safely
            for w in np.unique(np.round(weights[sel, k], 4)):
                pass
            g = groups[int(u)]
            ws = weights[sel, k]
            for val in np.unique(np.round(ws, 4)):
                g.add(sel[np.round(ws, 4) == val].tolist(), float(val), "ADD")
