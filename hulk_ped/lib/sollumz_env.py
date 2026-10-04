"""Make the Sollumz add-on available (headless ``bpy`` module or regular Blender)."""
import importlib
import os
import sys


def prepare_addon_path(addon_dir=None):
    """Call BEFORE ``import bpy`` when using the pip ``bpy`` module with a Sollumz checkout.

    Links the checkout into a private Blender "user scripts" folder so ``addon_enable("Sollumz")`` can find it."""
    addon_dir = addon_dir or os.environ.get("SOLLUMZ_DIR")
    if not addon_dir:
        return None
    addon_dir = os.path.abspath(addon_dir)
    scripts = os.path.join(os.path.dirname(addon_dir), ".blender_user_scripts")
    os.makedirs(os.path.join(scripts, "addons"), exist_ok=True)
    link = os.path.join(scripts, "addons", "Sollumz")
    if not os.path.lexists(link):
        os.symlink(addon_dir, link)
    os.environ["BLENDER_USER_SCRIPTS"] = scripts
    return scripts


def enable_sollumz():
    import bpy

    for name in bpy.context.preferences.addons.keys():
        if "sollumz" in name.lower():
            return importlib.import_module(name)
    res = bpy.ops.preferences.addon_enable(module="Sollumz")
    if "FINISHED" not in res:
        raise RuntimeError("Could not enable the Sollumz add-on: install it in Blender or pass --sollumz <checkout>")
    return importlib.import_module("Sollumz")
