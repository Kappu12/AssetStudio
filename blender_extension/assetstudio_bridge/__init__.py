# SPDX-License-Identifier: GPL-3.0-or-later
"""Standalone Facepunch Rust bridge. No Rust Tools or Ruster imports required."""
import json
import os
from pathlib import Path
import time
import uuid

import bpy
from bpy.app.handlers import persistent

PROTOCOL = 1
INSTANCE = uuid.uuid4().hex
_enabled = False
_last_status = "Waiting for AssetStudio"


def bridge_root():
    override = os.environ.get("ASSETSTUDIO_BLENDER_BRIDGE")
    if override:
        return Path(override)
    cache = os.environ.get("LOCALAPPDATA") or os.environ.get("XDG_CACHE_HOME")
    return (Path(cache) if cache else Path.home() / ".cache") / "AssetStudio" / "BlenderBridge"


def atomic_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps(value, ensure_ascii=False), encoding="utf-8")
    os.replace(temporary, path)


def package_path(directory, relative):
    if not isinstance(relative, str) or Path(relative).is_absolute():
        raise ValueError("Package paths must be relative")
    path = (directory / relative).resolve(strict=True)
    if not path.is_relative_to(directory.resolve()):
        raise ValueError("Package path escapes its export folder")
    return path


def rust_group():
    # Use our own copy, even when Rust Tools already has a group named Rust.
    for group in bpy.data.node_groups:
        if group.get("assetstudio_shader_version") == 1:
            return group
    with bpy.data.libraries.load(str(Path(__file__).with_name("rust_shader.blend")), link=False) as (_, data):
        data.node_groups = ["AssetStudio Rust"]
    group = data.node_groups[0]
    group["assetstudio_shader_version"] = 1
    return group


def build_material(material, description, directory):
    material.use_nodes = True
    material["assetstudio_original_name"] = description["name"]
    material.name = description["name"]
    tree = material.node_tree
    tree.nodes.clear()
    shader = tree.nodes.new("ShaderNodeGroup")
    shader.node_tree = rust_group()
    shader.location = (200, 0)
    shader.label = "Rust · " + description["workflow"]
    shader.inputs["Unity Nmap"].default_value = int(description.get("unity_normal", True))
    output = tree.nodes.new("ShaderNodeOutputMaterial")
    output.location = (520, 0)
    tree.links.new(shader.outputs["Metal" if description["workflow"] == "metallic" else "Specular"], output.inputs["Surface"])
    color = description.get("colors", {}).get("_Color")
    if color:
        shader.inputs["Albedo Map"].default_value = color
    shader.inputs["NMap Strength"].default_value = description.get("floats", {}).get("_BumpScale", 1.0)
    sockets = {"albedo": ("Albedo Map", "Albedo Alpha"), "specular": ("Specular Map", "Specular Alpha"),
               "emission": ("Emission", None), "normal": ("Nmap", "Nmap Alpha"), "occlusion": ("AO", None)}
    for index, (channel, binding) in enumerate(description["channels"].items()):
        if channel not in sockets:
            continue
        path = package_path(directory, binding["file"])
        # A separate image datablock avoids changing another material's colour space.
        image = bpy.data.images.load(str(path), check_existing=False)
        image.colorspace_settings.name = "Non-Color" if channel in {"normal", "occlusion"} or (channel == "specular" and description["workflow"] == "metallic") else "sRGB"
        image.alpha_mode = "CHANNEL_PACKED"
        texture = tree.nodes.new("ShaderNodeTexImage")
        texture.image = image
        texture.label = channel + " · " + binding.get("source", "reference")
        texture.location = (-380, -index * 290)
        texture["assetstudio_channel"] = channel
        color_socket, alpha_socket = sockets[channel]
        tree.links.new(texture.outputs["Color"], shader.inputs[color_socket])
        if alpha_socket:
            tree.links.new(texture.outputs["Alpha"], shader.inputs[alpha_socket])
        if channel == "occlusion":
            shader.inputs["AO Strength"].default_value = 1.0
        scale = binding.get("scale", [1, 1])
        offset = binding.get("offset", [0, 0])
        if scale != [1, 1] or offset != [0, 0]:
            uv = tree.nodes.new("ShaderNodeTexCoord")
            uv.location = (-1000, -index * 290)
            mapping = tree.nodes.new("ShaderNodeVectorMath")
            mapping.operation = "MULTIPLY_ADD"
            mapping.location = (-730, -index * 290)
            mapping.inputs[1].default_value = (*scale, 1)
            mapping.inputs[2].default_value = (*offset, 0)
            tree.links.new(uv.outputs["UV"], mapping.inputs[0])
            tree.links.new(mapping.outputs["Vector"], texture.inputs["Vector"])
    material["assetstudio_unresolved"] = ", ".join(description.get("unresolved", []))


def import_package(fbx):
    manifest = json.loads(fbx.with_name("assetstudio-materials.json").read_text(encoding="utf-8"))
    if manifest.get("version") != 1 or manifest.get("game") != "facepunch-rust":
        raise ValueError("Unsupported material manifest")
    descriptions = {m["fbx_name"]: m for m in manifest["materials"]}
    # Validate every texture before touching the scene.
    for description in descriptions.values():
        for binding in description["channels"].values():
            package_path(fbx.parent, binding["file"])
    previous = set(bpy.data.objects)
    previous_materials = set(bpy.data.materials)
    previous_images = set(bpy.data.images)
    collection = bpy.data.collections.new("AssetStudio · " + fbx.stem)
    bpy.context.scene.collection.children.link(collection)
    try:
        bpy.ops.import_scene.fbx(filepath=str(fbx), use_image_search=False)
        imported = set(bpy.data.objects) - previous
        materials = {slot.material for obj in imported for slot in obj.material_slots if slot.material}
        mapped = 0
        for material in materials:
            name = material.name
            # Blender adds .001 etc when the same package is imported twice.
            description = descriptions.get(name) or descriptions.get(name.rsplit(".", 1)[0])
            if description:
                build_material(material, description, fbx.parent)
                mapped += 1
        for obj in imported:
            for owner in list(obj.users_collection):
                owner.objects.unlink(obj)
            collection.objects.link(obj)
        collection["assetstudio_source"] = str(fbx)
        warnings = [warning for m in descriptions.values() for warning in m.get("warnings", [])]
        return {"objects": len(imported), "materials": mapped, "warnings": warnings,
                "unresolved": {m["name"]: m.get("unresolved", []) for m in descriptions.values() if m.get("unresolved")}}
    except Exception:
        # Only newly imported objects/data are eligible for cleanup.
        for obj in set(bpy.data.objects) - previous:
            bpy.data.objects.remove(obj, do_unlink=True)
        for material in set(bpy.data.materials) - previous_materials:
            if material.users == 0:
                bpy.data.materials.remove(material)
        for image in set(bpy.data.images) - previous_images:
            if image.users == 0:
                bpy.data.images.remove(image)
        bpy.data.collections.remove(collection)
        raise


def process_job(job):
    request = json.loads((job / "request.ready.json").read_text(encoding="utf-8"))
    if request.get("version") != PROTOCOL or request.get("receiver") != INSTANCE:
        return
    # Claim first. A reload or failed receipt write must never duplicate an import.
    os.replace(job / "request.ready.json", job / "request.processing.json")
    results, failures = [], []
    selection = list(bpy.context.selected_objects)
    active = bpy.context.view_layer.objects.active
    try:
        for index, relative in enumerate(request["models"]):
            atomic_json(job / "progress.json", {"completed": index, "total": len(request["models"])})
            try:
                fbx = package_path(job, relative)
                if fbx.suffix.lower() != ".fbx":
                    raise ValueError("Only FBX model packages are supported")
                results.append(import_package(fbx))
            except Exception as error:
                failures.append(f"{relative}: {error}")
    finally:
        for obj in bpy.context.selected_objects:
            obj.select_set(False)
        for obj in selection:
            if obj.name in bpy.context.view_layer.objects:
                obj.select_set(True)
        bpy.context.view_layer.objects.active = active
    atomic_json(job / "result.json", {"version": PROTOCOL, "imported": results, "failed": failures})
    return results, failures


class ASSETSTUDIO_OT_receive(bpy.types.Operator):
    bl_idname = "assetstudio.receive"
    bl_label = "Import from AssetStudio"
    bl_options = {'REGISTER', 'UNDO', 'INTERNAL'}

    directory: bpy.props.StringProperty(subtype='DIR_PATH', options={'HIDDEN'})

    def execute(self, context):
        global _last_status
        outcome = process_job(Path(self.directory))
        if outcome:
            results, failures = outcome
            _last_status = f"Imported {len(results)} model(s)" + (f" · {len(failures)} failed" if failures else "")
        return {'FINISHED'}


def tick():
    global _last_status
    if not _enabled:
        return None
    root = bridge_root()
    try:
        atomic_json(root / "receivers" / (INSTANCE + ".json"), {"version": PROTOCOL, "receiver": INSTANCE,
                    "time": time.time(), "blender": bpy.app.version_string, "scene": bpy.context.scene.name})
        # Imports use Blender operators on the main thread, in Object Mode only.
        if bpy.context.mode == "OBJECT":
            for ready in sorted((root / "jobs").glob("*/request.ready.json")):
                try:
                    request = json.loads(ready.read_text(encoding="utf-8"))
                    if request.get("receiver") == INSTANCE:
                        bpy.ops.assetstudio.receive(directory=str(ready.parent))
                        break
                except Exception as error:
                    _last_status = str(error)
                    atomic_json(ready.parent / "result.json", {"version": PROTOCOL, "imported": [], "failed": [str(error)]})
    except Exception as error:
        _last_status = str(error)
    return 1.0


class ASSETSTUDIO_PT_bridge(bpy.types.Panel):
    bl_label = "AssetStudio Rust Bridge"
    bl_idname = "ASSETSTUDIO_PT_bridge"
    bl_space_type = "VIEW_3D"
    bl_region_type = "UI"
    bl_category = "AssetStudio"

    def draw(self, context):
        self.layout.label(text="Connected · waiting for exports", icon="LINKED")
        self.layout.label(text="Use Export to Blender in AssetStudio")
        self.layout.label(text=_last_status)
        if context.mode != "OBJECT":
            self.layout.label(text="Switch to Object Mode to receive models", icon="INFO")


@persistent
def resume_bridge(_):
    if _enabled and not bpy.app.timers.is_registered(tick):
        bpy.app.timers.register(tick, first_interval=1.0, persistent=True)


def register():
    global _enabled
    bpy.utils.register_class(ASSETSTUDIO_OT_receive)
    bpy.utils.register_class(ASSETSTUDIO_PT_bridge)
    _enabled = True
    bpy.app.handlers.load_post.append(resume_bridge)
    bpy.app.timers.register(tick, first_interval=1.0, persistent=True)


def unregister():
    global _enabled
    _enabled = False
    if bpy.app.timers.is_registered(tick):
        bpy.app.timers.unregister(tick)
    if resume_bridge in bpy.app.handlers.load_post:
        bpy.app.handlers.load_post.remove(resume_bridge)
    (bridge_root() / "receivers" / (INSTANCE + ".json")).unlink(missing_ok=True)
    bpy.utils.unregister_class(ASSETSTUDIO_PT_bridge)
    bpy.utils.unregister_class(ASSETSTUDIO_OT_receive)
