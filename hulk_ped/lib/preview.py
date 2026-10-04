"""Quick multi-angle preview renders of raw numpy meshes (Cycles, CPU)."""
import math
import os

import numpy as np


def mesh_from_arrays(name, verts, faces):
    import bpy

    me = bpy.data.meshes.new(name)
    me.vertices.add(len(verts))
    me.vertices.foreach_set("co", np.asarray(verts, np.float32).ravel())
    nf = len(faces)
    faces = np.asarray(faces, np.int32)
    me.loops.add(nf * 3)
    me.polygons.add(nf)
    me.loops.foreach_set("vertex_index", faces.ravel())
    me.polygons.foreach_set("loop_start", np.arange(0, nf * 3, 3, dtype=np.int32))
    me.update(calc_edges=True)
    me.validate()
    return me


def flat_mat(name, color, rough=0.55):
    import bpy

    m = bpy.data.materials.new(name)
    if bpy.app.version < (5, 0, 0):
        m.use_nodes = True
    b = m.node_tree.nodes["Principled BSDF"]
    b.inputs["Base Color"].default_value = (*color, 1.0)
    b.inputs["Roughness"].default_value = rough
    return m


def render_montage(objs, out_png, views=("front", "three_quarter", "side", "back"), size=(480, 760),
                   samples=24, focus=(0.0, 0.0, -0.1), dist=3.2, ortho_scale=None, extra_setup=None, cam_height=None):
    """``objs``: list of bpy objects already in the scene.  Renders each view and tiles them into ``out_png``."""
    import bpy
    from PIL import Image

    out_png = os.path.abspath(out_png)
    scene = bpy.context.scene
    scene.render.engine = "CYCLES"
    scene.cycles.device = "CPU"
    scene.cycles.samples = samples
    scene.cycles.use_denoising = True
    scene.render.resolution_x, scene.render.resolution_y = size
    scene.render.resolution_percentage = 100
    scene.view_settings.view_transform = "Standard"

    world = bpy.data.worlds.get("W") or bpy.data.worlds.new("W")
    if bpy.app.version < (5, 0, 0):
        world.use_nodes = True
    bg = world.node_tree.nodes["Background"]
    bg.inputs["Color"].default_value = (0.42, 0.45, 0.5, 1)
    bg.inputs["Strength"].default_value = 1.0
    scene.world = world

    if bpy.data.objects.get("Sun") is None:
        sun = bpy.data.objects.new("Sun", bpy.data.lights.new("Sun", "SUN"))
        sun.data.energy = 3.0
        sun.rotation_euler = (math.radians(55), math.radians(10), math.radians(-35))
        scene.collection.objects.link(sun)
        fill = bpy.data.objects.new("Fill", bpy.data.lights.new("Fill", "SUN"))
        fill.data.energy = 1.2
        fill.rotation_euler = (math.radians(70), 0, math.radians(140))
        scene.collection.objects.link(fill)

    cam = bpy.data.objects.get("Cam")
    if cam is None:
        cam_data = bpy.data.cameras.new("Cam")
        cam_data.lens = 70
        cam = bpy.data.objects.new("Cam", cam_data)
        scene.collection.objects.link(cam)
    cam_data = cam.data
    scene.camera = cam

    if extra_setup:
        extra_setup()

    azim = {"front": 0, "three_quarter": 38, "side": 90, "back": 180, "three_quarter_back": 142, "left": -90}
    tiles = []
    fx, fy, fz = focus
    for v in views:
        a = math.radians(azim[v] if isinstance(v, str) else v)
        # the character faces -Y, so "front" puts the camera at -Y
        cam.location = (fx + math.sin(a) * dist, fy - math.cos(a) * dist, fz + (cam_height or 0.0))
        d = np.array([fx, fy, fz]) - np.array(cam.location)
        cam.rotation_euler = _look(d)
        if ortho_scale:
            cam_data.type = "ORTHO"
            cam_data.ortho_scale = ortho_scale
        path = out_png + f".{v}.png"
        scene.render.filepath = path
        bpy.ops.render.render(write_still=True)
        tiles.append(Image.open(path).convert("RGB"))
        os.remove(path)

    w, h = tiles[0].size
    sheet = Image.new("RGB", (w * len(tiles), h))
    for i, t in enumerate(tiles):
        sheet.paste(t, (i * w, 0))
    sheet.save(out_png)
    return out_png


def _look(direction):
    from mathutils import Vector

    return Vector(direction).to_track_quat("-Z", "Y").to_euler()


def mesh_with_uv(name, verts, faces, uv):
    """Mesh with per-vertex UVs (uv indexed like verts)."""
    me = mesh_from_arrays(name, verts, faces)
    layer = me.uv_layers.new(name="UVMap")
    loops = np.asarray(faces, np.int32).ravel()
    layer.data.foreach_set("uv", np.asarray(uv, np.float32)[loops].ravel())
    return me


def checker_image(name="checker", size=1024, cells=32):
    import bpy

    yy, xx = np.mgrid[0:size, 0:size]
    c = ((xx // (size // cells)) + (yy // (size // cells))) % 2
    r = np.where(c == 0, 0.9, 0.15)
    g = np.where(c == 0, 0.9, 0.35) * (0.6 + 0.4 * xx / size)
    b = np.where(c == 0, 0.9, 0.7) * (0.6 + 0.4 * yy / size)
    px = np.stack([r, g, b, np.ones_like(r)], axis=-1).astype(np.float32)
    img = bpy.data.images.new(name, size, size, alpha=False)
    img.pixels.foreach_set(px.ravel())
    img.pack()
    return img


def textured_mat(name, img):
    import bpy

    m = bpy.data.materials.new(name)
    if bpy.app.version < (5, 0, 0):
        m.use_nodes = True
    nt = m.node_tree
    tex = nt.nodes.new("ShaderNodeTexImage")
    tex.image = img
    nt.links.new(tex.outputs["Color"], nt.nodes["Principled BSDF"].inputs["Base Color"])
    nt.nodes["Principled BSDF"].inputs["Roughness"].default_value = 0.8
    return m
