"""
Procedural Hulk-style character for Blender.

Builds a stylised green muscle giant in torn blue jeans (based on the
reference image) out of metaball "muscle" volumes, converts them to
meshes, applies materials, and saves/exports the result.

Run inside Blender:
    blender --background --python build_hulk.py
or with the `bpy` pip module:
    python build_hulk.py [--render]

Outputs (next to this script):
    hulk.blend, hulk.glb, hulk.obj (+ .mtl), and optionally hulk_render.png
"""
import math
import os
import sys

import bpy
from mathutils import Vector

HERE = os.path.dirname(os.path.abspath(__file__))
RENDER = "--render" in sys.argv


# ---------------------------------------------------------------------------
# Scene helpers
# ---------------------------------------------------------------------------
def reset_scene():
    bpy.ops.wm.read_factory_settings(use_empty=True)


def new_metaball(name, resolution=0.025):
    mb = bpy.data.metaballs.new(name)
    mb.resolution = resolution
    mb.render_resolution = resolution
    mb.threshold = THRESHOLD
    obj = bpy.data.objects.new(name, mb)
    bpy.context.collection.objects.link(obj)
    return obj


THRESHOLD = 0.6
STIFFNESS = 8.0


def field_radius(r, stiffness):
    """Metaball radius whose isolated surface sits at distance r.

    Blender's field is stiffness * (1 - d^2/R^2)^3, so the surface of a
    lone element is at d = R * sqrt(1 - (threshold / stiffness)^(1/3)).
    Specifying the visible radius keeps the anatomy numbers intuitive.
    """
    return r / math.sqrt(1 - (THRESHOLD / stiffness) ** (1 / 3))


def ball(mb, co, r, size=None, stiffness=STIFFNESS):
    """Ball or ellipsoid element. `size` are per-axis scale factors."""
    el = mb.data.elements.new(type="ELLIPSOID" if size else "BALL")
    el.co = co
    el.radius = field_radius(r, stiffness)
    el.stiffness = stiffness
    if size:
        el.size_x, el.size_y, el.size_z = size
    return el


def limb(mb, a, b, r, stiffness=STIFFNESS):
    """Capsule element stretched between points a and b."""
    a, b = Vector(a), Vector(b)
    d = b - a
    el = mb.data.elements.new(type="CAPSULE")
    el.co = (a + b) / 2
    el.radius = field_radius(r, stiffness)
    el.stiffness = stiffness
    el.size_x = d.length / 2
    el.rotation = Vector((1, 0, 0)).rotation_difference(d.normalized())
    return el


def both(fn, *args, **kw):
    """Call fn for the left (+x) and mirrored right (-x) side."""
    def mirror(v):
        return (-v[0], v[1], v[2]) if isinstance(v, tuple) and len(v) == 3 else v
    fn(*args, **kw)
    fn(*[mirror(a) for a in args], **kw)


def to_mesh(obj, name):
    """Bake a metaball family into a real mesh object."""
    dg = bpy.context.evaluated_depsgraph_get()
    me = bpy.data.meshes.new_from_object(obj.evaluated_get(dg))
    me.name = name
    for p in me.polygons:
        p.use_smooth = True
    new = bpy.data.objects.new(name, me)
    bpy.context.collection.objects.link(new)
    return new


# ---------------------------------------------------------------------------
# Materials
# ---------------------------------------------------------------------------
def principled(name, color, rough=0.5, sss=0.0):
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    bsdf = mat.node_tree.nodes["Principled BSDF"]
    bsdf.inputs["Base Color"].default_value = (*color, 1)
    bsdf.inputs["Roughness"].default_value = rough
    if sss:
        bsdf.inputs["Subsurface Weight"].default_value = sss
        bsdf.inputs["Subsurface Radius"].default_value = (0.3, 0.6, 0.2)
        bsdf.inputs["Subsurface Scale"].default_value = 0.02
    return mat, bsdf


def skin_material():
    mat, bsdf = principled("HulkSkin", (0.16, 0.42, 0.04), rough=0.45, sss=0.15)
    nt = mat.node_tree
    # subtle mottling so the skin isn't flat plastic
    noise = nt.nodes.new("ShaderNodeTexNoise")
    noise.inputs["Scale"].default_value = 18
    ramp = nt.nodes.new("ShaderNodeValToRGB")
    ramp.color_ramp.elements[0].color = (0.05, 0.17, 0.01, 1)
    ramp.color_ramp.elements[1].color = (0.13, 0.33, 0.03, 1)
    nt.links.new(noise.outputs["Fac"], ramp.inputs["Fac"])
    nt.links.new(ramp.outputs["Color"], bsdf.inputs["Base Color"])
    bump = nt.nodes.new("ShaderNodeBump")
    bump.inputs["Strength"].default_value = 0.08
    nt.links.new(noise.outputs["Fac"], bump.inputs["Height"])
    nt.links.new(bump.outputs["Normal"], bsdf.inputs["Normal"])
    return mat


def denim_material():
    mat, bsdf = principled("Denim", (0.18, 0.32, 0.5), rough=0.9)
    nt = mat.node_tree
    # faded denim: light/dark patches plus a fine weave bump
    noise = nt.nodes.new("ShaderNodeTexNoise")
    noise.inputs["Scale"].default_value = 4
    ramp = nt.nodes.new("ShaderNodeValToRGB")
    ramp.color_ramp.elements[0].color = (0.04, 0.09, 0.2, 1)
    ramp.color_ramp.elements[1].color = (0.18, 0.28, 0.42, 1)
    nt.links.new(noise.outputs["Fac"], ramp.inputs["Fac"])
    nt.links.new(ramp.outputs["Color"], bsdf.inputs["Base Color"])
    wave = nt.nodes.new("ShaderNodeTexWave")
    wave.inputs["Scale"].default_value = 300
    bump = nt.nodes.new("ShaderNodeBump")
    bump.inputs["Strength"].default_value = 0.15
    nt.links.new(wave.outputs["Fac"], bump.inputs["Height"])
    nt.links.new(bump.outputs["Normal"], bsdf.inputs["Normal"])
    return mat


# ---------------------------------------------------------------------------
# Character (faces -Y, feet on z=0, ~2.4 m tall, left leg striding forward)
# ---------------------------------------------------------------------------
def build_body():
    b = new_metaball("BodyMeta")

    # head, jaw, brow
    ball(b, (0, 0.0, 2.2), 0.17, (0.85, 0.95, 1.0))           # skull
    ball(b, (0, -0.05, 2.08), 0.14, (1.05, 0.85, 0.8))         # heavy jaw
    ball(b, (0, -0.14, 2.21), 0.1, (1.3, 0.4, 0.3))            # brow ridge
    both(ball, b, (0.08, -0.13, 2.12), 0.05)                   # cheekbones
    ball(b, (0, -0.18, 2.15), 0.04, (0.8, 0.9, 1.2))           # nose
    ball(b, (0, -0.15, 2.04), 0.06, (1.3, 0.6, 0.6))           # chin
    # neck + massive traps
    limb(b, (0, 0.03, 1.9), (0, 0.0, 2.06), 0.13)
    ball(b, (0, 0.07, 1.84), 0.3, (1.6, 0.7, 0.45))
    # ribcage / back / lats
    ball(b, (0, 0.04, 1.6), 0.46, (1.15, 0.75, 0.95))
    both(ball, b, (0.3, 0.08, 1.48), 0.28, (0.85, 0.8, 1.3))
    # pecs
    both(ball, b, (0.19, -0.16, 1.66), 0.25, (1.05, 0.65, 0.75))
    # abdomen + abs
    ball(b, (0, -0.04, 1.32), 0.34, (0.95, 0.75, 1.0))
    for z in (1.42, 1.32, 1.22):
        both(ball, b, (0.075, -0.24, z), 0.075, stiffness=3.0)
    # obliques / pelvis (mostly hidden by jeans)
    both(ball, b, (0.22, -0.02, 1.2), 0.2, (0.9, 0.9, 1.2))
    ball(b, (0, 0.0, 1.08), 0.34, (1.15, 0.8, 0.7))

    # shoulders + arms (slightly bent, hands open beside thighs)
    both(ball, b, (0.55, 0.0, 1.78), 0.25)                     # deltoid
    both(ball, b, (0.4, -0.08, 1.74), 0.19)                    # pec/delt tie-in
    both(limb, b, (0.6, 0.0, 1.72), (0.74, 0.02, 1.36), 0.17)  # upper arm
    both(ball, b, (0.66, -0.07, 1.56), 0.15)                   # bicep
    both(ball, b, (0.69, 0.08, 1.58), 0.14)                    # tricep
    both(limb, b, (0.74, 0.02, 1.36), (0.8, -0.06, 1.0), 0.12) # forearm
    both(ball, b, (0.76, -0.01, 1.28), 0.15, (1.0, 1.0, 1.25)) # forearm bulk
    both(ball, b, (0.82, -0.07, 0.88), 0.13, (0.6, 1.0, 1.1))  # palm
    for i, dy in enumerate((-0.15, -0.1, -0.05, 0.0)):         # fingers
        both(limb, b, (0.83, dy, 0.8), (0.85 - 0.01 * i, dy - 0.05, 0.66), 0.042)
    both(limb, b, (0.78, -0.15, 0.92), (0.76, -0.23, 0.83), 0.045)  # thumb

    # legs: left (+x) forward, right back
    hips = {1: (0.22, 0.0, 1.0), -1: (-0.22, 0.0, 1.0)}
    knees = {1: (0.26, -0.18, 0.56), -1: (-0.25, 0.12, 0.58)}
    ankles = {1: (0.27, -0.2, 0.12), -1: (-0.27, 0.28, 0.15)}
    for s in (1, -1):
        limb(b, hips[s], knees[s], 0.21)
        limb(b, knees[s], ankles[s], 0.14)
        k, a = Vector(knees[s]), Vector(ankles[s])
        ball(b, tuple(k.lerp(a, 0.35) + Vector((0, 0.06, 0))), 0.16, (1, 1, 1.4))  # calf
        ball(b, (a.x, a.y - 0.1, 0.07), 0.13, (0.85, 1.7, 0.55))                   # foot
        for i, dx in enumerate((-0.07, -0.025, 0.02, 0.06)):                       # toes
            ball(b, (a.x + s * dx, a.y - 0.29 + 0.012 * i, 0.04), 0.04)
    return b, ankles, knees, hips


def build_jeans(hips, knees, ankles):
    j = new_metaball("JeansMeta")
    ball(j, (0, 0.0, 1.06), 0.38, (1.12, 0.85, 0.5))
    for s in (1, -1):
        h, k, a = Vector(hips[s]), Vector(knees[s]), Vector(ankles[s])
        limb(j, tuple(h), tuple(k), 0.235)
        # torn hem: forward leg ripped to mid-shin, back leg longer
        hem = k.lerp(a, 0.55 if s == 1 else 0.75)
        limb(j, tuple(k), tuple(hem), 0.17)
    return j


def build_hair():
    h = new_metaball("HairMeta", resolution=0.015)
    ball(h, (0, 0.02, 2.29), 0.16, (0.92, 0.98, 0.5))
    ball(h, (0, 0.07, 2.22), 0.16, (0.92, 0.75, 0.8))
    ball(h, (0, -0.08, 2.32), 0.08, (1.3, 0.8, 0.5))  # front tuft
    return h


def build_eyes():
    mat, _ = principled("Eyes", (0.02, 0.02, 0.02), rough=0.15)
    eyes = []
    for x in (0.055, -0.055):
        bpy.ops.mesh.primitive_uv_sphere_add(radius=0.018, location=(x, -0.16, 2.165))
        e = bpy.context.active_object
        e.name = "Eye.L" if x > 0 else "Eye.R"
        e.data.materials.append(mat)
        bpy.ops.object.shade_smooth()
        eyes.append(e)
    return eyes


def build_character():
    body_mb, ankles, knees, hips = build_body()
    jeans_mb = build_jeans(hips, knees, ankles)
    hair_mb = build_hair()

    body = to_mesh(body_mb, "Hulk_Body")
    jeans = to_mesh(jeans_mb, "Hulk_Jeans")
    hair = to_mesh(hair_mb, "Hulk_Hair")
    for mb in (body_mb, jeans_mb, hair_mb):
        bpy.data.objects.remove(mb)

    body.data.materials.append(skin_material())
    jeans.data.materials.append(denim_material())
    hair.data.materials.append(principled("Hair", (0.02, 0.02, 0.018), rough=0.6)[0])

    for o in (body, jeans, hair):
        sm = o.modifiers.new("Smooth", "CORRECTIVE_SMOOTH")
        sm.iterations = 8
        sm.smooth_type = "LENGTH_WEIGHTED"
        sm.use_only_smooth = True

    parts = [body, jeans, hair] + build_eyes()
    root = bpy.data.objects.new("Hulk", None)
    bpy.context.collection.objects.link(root)
    for p in parts:
        p.parent = root
    return root, parts


# ---------------------------------------------------------------------------
# Stage (camera, lights, road) for a preview render
# ---------------------------------------------------------------------------
def build_stage():
    scene = bpy.context.scene
    bpy.ops.mesh.primitive_plane_add(size=30)
    road = bpy.context.active_object
    road.name = "Road"
    road.data.materials.append(principled("Asphalt", (0.09, 0.09, 0.1), rough=0.95)[0])

    cam_data = bpy.data.cameras.new("Camera")
    cam_data.lens = 50
    cam = bpy.data.objects.new("Camera", cam_data)
    scene.collection.objects.link(cam)
    cam.location = (1.9, -5.6, 1.6)
    target = Vector((0, 0, 1.2))
    cam.rotation_euler = (target - cam.location).to_track_quat("-Z", "Y").to_euler()
    scene.camera = cam

    sun = bpy.data.objects.new("Sun", bpy.data.lights.new("Sun", "SUN"))
    sun.data.energy = 3.5
    sun.rotation_euler = (math.radians(50), 0, math.radians(30))
    scene.collection.objects.link(sun)
    fill = bpy.data.objects.new("Fill", bpy.data.lights.new("Fill", "AREA"))
    fill.data.energy = 600
    fill.data.size = 4
    fill.location = (-4, -3, 3)
    fill.rotation_euler = (target - fill.location).to_track_quat("-Z", "Y").to_euler()
    scene.collection.objects.link(fill)

    world = bpy.data.worlds.new("World")
    world.use_nodes = True
    world.node_tree.nodes["Background"].inputs["Color"].default_value = (0.55, 0.62, 0.7, 1)
    world.node_tree.nodes["Background"].inputs["Strength"].default_value = 0.6
    scene.world = world

    scene.render.engine = "CYCLES"
    scene.cycles.device = "CPU"
    scene.cycles.samples = 48
    scene.cycles.use_denoising = True
    scene.render.resolution_x = 720
    scene.render.resolution_y = 1080
    scene.view_settings.view_transform = "Standard"
    return road, cam, sun, fill


def main():
    reset_scene()
    root, parts = build_character()
    stage = build_stage()

    bpy.ops.wm.save_as_mainfile(filepath=os.path.join(HERE, "hulk.blend"))

    # export the character only (no road / camera / lights)
    bpy.ops.object.select_all(action="DESELECT")
    for p in parts:
        p.select_set(True)
    bpy.context.view_layer.objects.active = parts[0]
    bpy.ops.export_scene.gltf(filepath=os.path.join(HERE, "hulk.glb"),
                              use_selection=True, export_apply=True)
    bpy.ops.wm.obj_export(filepath=os.path.join(HERE, "hulk.obj"),
                          export_selected_objects=True, apply_modifiers=True)

    if RENDER:
        bpy.context.scene.render.filepath = os.path.join(HERE, "hulk_render.png")
        bpy.ops.render.render(write_still=True)


if __name__ == "__main__":
    main()
