"""World-space posing of the ped armature(s) for showcase renders."""
import math


def rotate_about_joint(arm_obj, bone, axis, degrees, pivot=None):
    """Rotate ``bone`` (and, through the hierarchy, its children) about ``axis`` through the bone's head (or ``pivot``)."""
    import bpy
    from mathutils import Matrix, Vector

    pb = arm_obj.pose.bones[bone]
    piv = Vector(pivot) if pivot is not None else pb.bone.head_local
    R = Matrix.Translation(piv) @ Matrix.Rotation(math.radians(degrees), 4, Vector(axis)) @ Matrix.Translation(-piv)
    pb.matrix = R @ pb.matrix
    bpy.context.view_layer.update()


HERO_POSE = [
    # (bone, axis, degrees) applied in order; world-space rotations about each bone's head
    ("SKEL_Spine1", (1, 0, 0), -8), ("SKEL_Spine2", (1, 0, 0), -8),
    ("SKEL_Head", (1, 0, 0), 10),
    ("SKEL_L_UpperArm", (0, 1, 0), -24), ("SKEL_L_Forearm", (1, 0, 0), -52), ("SKEL_L_Hand", (1, 0, 0), -18),
    ("SKEL_R_UpperArm", (0, 1, 0), 24), ("SKEL_R_Forearm", (1, 0, 0), -52), ("SKEL_R_Hand", (1, 0, 0), -18),
    ("SKEL_L_Thigh", (0, 1, 0), -15), ("SKEL_L_Foot", (0, 1, 0), 15),
    ("SKEL_R_Thigh", (0, 1, 0), 15), ("SKEL_R_Foot", (0, 1, 0), -15),
]


def apply_pose(arm_objs, pose=HERO_POSE):
    import bpy

    for arm in arm_objs:
        bpy.context.view_layer.objects.active = arm
        bpy.ops.object.mode_set(mode="POSE")
        for bone, axis, deg in pose:
            rotate_about_joint(arm, bone, axis, deg)
        bpy.ops.object.mode_set(mode="OBJECT")


def clear_pose(arm_objs):
    import bpy

    for arm in arm_objs:
        for pb in arm.pose.bones:
            pb.location = (0, 0, 0)
            pb.rotation_quaternion = (1, 0, 0, 0)
            pb.rotation_euler = (0, 0, 0)
            pb.scale = (1, 1, 1)
    bpy.context.view_layer.update()
