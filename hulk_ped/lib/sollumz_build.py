"""Build the Sollumz object hierarchy for a ped component (Drawable Dictionary -> Drawable -> Drawable Model)."""
import numpy as np


def _import():
    from Sollumz.sollumz_properties import LODLevel, SollumType
    from Sollumz.tools.blenderhelper import add_armature_modifier, create_blender_object, create_empty_object
    from Sollumz.ydr.shader_materials import create_shader

    return LODLevel, SollumType, add_armature_modifier, create_blender_object, create_empty_object, create_shader


def ped_material(name, shader, diffuse=None, normal=None, spec=None, params=None):
    """Sollumz shader material.  ``diffuse``: external texture, ``normal`` / ``spec``: embedded in the drawable."""
    create_shader = _import()[-1]
    mat = create_shader(shader)
    mat.name = name
    nodes = mat.node_tree.nodes
    for node_name, img, embedded in (("DiffuseSampler", diffuse, False), ("BumpSampler", normal, True), ("SpecSampler", spec, True)):
        if img is None or node_name not in nodes:
            continue
        nodes[node_name].image = img
        nodes[node_name].texture_properties.embedded = embedded
        if node_name != "DiffuseSampler":
            img.colorspace_settings.is_data = True
    for pname, vals in (params or {}).items():
        node = nodes.get(pname)
        if node is None:
            continue
        for comp, v in zip("XYZW", vals):
            node.set(comp, v)
    return mat


def mesh_from_part(name, verts, tris, uv, normals, mat_index=None):
    """Blender mesh with the attributes Sollumz expects for ped shaders (UVMap 0, Color 1, Color 2, split normals)."""
    import bpy

    me = bpy.data.meshes.new(name)
    nv, nf = len(verts), len(tris)
    me.vertices.add(nv)
    me.vertices.foreach_set("co", np.asarray(verts, np.float32).ravel())
    me.loops.add(nf * 3)
    me.polygons.add(nf)
    me.loops.foreach_set("vertex_index", np.asarray(tris, np.int32).ravel())
    me.polygons.foreach_set("loop_start", np.arange(0, nf * 3, 3, dtype=np.int32))
    me.update(calc_edges=True)
    if mat_index is not None:
        me.polygons.foreach_set("material_index", np.asarray(mat_index, np.int32))
    for p in me.polygons:
        p.use_smooth = True
    loop_uv = np.asarray(uv, np.float32)[np.asarray(tris, np.int32).ravel()].ravel()
    for uv_name in ("UVMap 0", "UVMap 1"):   # ped.sps declares a second UV channel; mirror the first
        me.uv_layers.new(name=uv_name).data.foreach_set("uv", loop_uv)
    for cname in ("Color 1", "Color 2"):
        attr = me.color_attributes.new(name=cname, type="BYTE_COLOR", domain="CORNER")
        attr.data.foreach_set("color", np.ones(nf * 3 * 4, np.float32))
    me.normals_split_custom_set_from_vertices([tuple(x) for x in np.asarray(normals, np.float32)])
    return me


def build_component(dict_obj, comp, lod_meshes, materials, group_names, lod_weights, armature_obj):
    """One ped component.

    ``lod_meshes``: {LODLevel: Mesh}, ``lod_weights``: {LODLevel: (weights (N,4), bone-name-index (N,4))} where the
    index addresses ``group_names``.  Returns the Drawable Model object.
    """
    LODLevel, SollumType, add_armature_modifier, create_blender_object, create_empty_object, _ = _import()
    drawable = create_empty_object(SollumType.DRAWABLE, comp)
    drawable.parent = dict_obj
    model = create_blender_object(SollumType.DRAWABLE_MODEL, f"{comp}.model")
    model.parent = drawable
    lods = model.sz_lods
    groups = {}
    first = True
    for level in (LODLevel.HIGH, LODLevel.MEDIUM, LODLevel.LOW, LODLevel.VERYLOW):
        me = lod_meshes.get(level)
        if me is None:
            continue
        for m in materials:
            me.materials.append(m)
        lods.get_lod(level).mesh = me
        lods.active_lod_level = level
        if first:
            first = False
        wts, bidx = lod_weights[level]
        for k in range(4):
            for b in np.unique(bidx[:, k]):
                sel = np.where((bidx[:, k] == b) & (wts[:, k] > 0))[0]
                if len(sel) == 0:
                    continue
                nm = group_names[int(b)]
                g = groups.get(nm) or model.vertex_groups.new(name=nm)
                groups[nm] = g
                vals = np.round(wts[sel, k], 4)
                for val in np.unique(vals):
                    g.add(sel[vals == val].tolist(), float(val), "ADD")
    lods.set_highest_lod_active()
    add_armature_modifier(model, armature_obj)
    return drawable, model
