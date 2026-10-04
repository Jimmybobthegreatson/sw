import os, sys, pickle, math
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
HERE = os.path.dirname(os.path.abspath(__file__))
import numpy as np
SC = "/tmp/claude-0/-home-user-sw/bbe8b9d2-6c61-5aa1-8f13-5b7a5094ca7b/scratchpad/"
os.environ["BLENDER_USER_SCRIPTS"] = SC + "blscripts"
import bpy
bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.preferences.addon_enable(module="Sollumz")
from mathutils import Matrix, Vector
from lib.skeleton import Joints
from lib import ped_rig, skinning
from lib.preview import mesh_from_arrays, flat_mat, render_montage
J = Joints()
res = pickle.load(open(SC + "parts_cache.pkl", "rb"))
arm = ped_rig.create_armature("rig")
cols = {"uppr": (0.2, 0.5, 0.1), "head": (0.2, 0.5, 0.1), "hand": (0.2, 0.5, 0.1), "feet": (0.2, 0.5, 0.1), "jeans": (0.1, 0.2, 0.45), "hair": (0.03, 0.03, 0.03)}
for name, col in cols.items():
    v, f = res[name]
    kind = "head" if name == "head" else ("hair" if name == "hair" else "body")
    names, wt, bi = skinning.compute_weights(v, f, J, kind=kind)
    me = mesh_from_arrays(name, v, f)
    for p in me.polygons: p.use_smooth = True
    ob = bpy.data.objects.new(name, me); bpy.context.collection.objects.link(ob)
    ped_rig.assign_weights(ob, names, wt, bi)
    mod = ob.modifiers.new("arm", "ARMATURE"); mod.object = arm
    me.materials.append(flat_mat(name, col))
    print(name, "groups", len(ob.vertex_groups))
# pose: rotate about joints in world space
bpy.context.view_layer.objects.active = arm
bpy.ops.object.mode_set(mode="POSE")
pb = arm.pose.bones
def descendants(b):
    out = [b]
    for c in b.children: out += descendants(c)
    return out
def rotate(bone, axis, deg, pivot=None):
    b = pb[bone]
    piv = Vector(pivot) if pivot is not None else b.bone.head_local
    R = Matrix.Translation(piv) @ Matrix.Rotation(math.radians(deg), 4, Vector(axis)) @ Matrix.Translation(-piv)
    b.matrix = R @ b.matrix
    bpy.context.view_layer.update()
# raise left arm sideways ~60deg, bend elbow, right arm forward, knee bends, spine twist
rotate("SKEL_L_UpperArm", (0, 1, 0), -55)
rotate("SKEL_L_Forearm", (0, 0, 1), -35)
rotate("SKEL_R_UpperArm", (1, 0, 0), -80)
rotate("SKEL_R_Forearm", (1, 0, 0), -70)
rotate("SKEL_L_Thigh", (1, 0, 0), -55)
rotate("SKEL_L_Calf", (1, 0, 0), 70)
rotate("SKEL_Spine2", (0, 0, 1), 28)
rotate("SKEL_Head", (0, 0, 1), -25)
bpy.ops.object.mode_set(mode="OBJECT")
g = bpy.data.objects.new("g", mesh_from_arrays("g", np.array([[-3,-3,-1.0],[3,-3,-1.0],[3,3,-1.0],[-3,3,-1.0]],np.float32), np.array([[0,1,2],[0,2,3]],np.int32)))
bpy.context.collection.objects.link(g); g.data.materials.append(flat_mat("f",(0.25,0.25,0.27),0.9))
arm.hide_render = True
render_montage([], os.path.join(HERE, "pose.png"), views=["front", "three_quarter", "side", "back"], focus=(0, 0, -0.1), dist=4.8, samples=16, size=(480, 760))
