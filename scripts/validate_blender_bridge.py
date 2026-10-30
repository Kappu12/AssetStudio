"""Run with Blender --factory-startup --python, no other extensions enabled.

Receives one real AssetStudio export, checks its materials and repeat import,
then writes evidence. Set ASSETSTUDIO_BLENDER_BRIDGE to an isolated queue.
"""
import importlib.util
import json
import os
from pathlib import Path
import sys
import time
import shutil
import tempfile

import bpy

ROOT = Path(__file__).resolve().parents[1]
source = ROOT / "blender_extension" / "assetstudio_bridge" / "__init__.py"
spec = importlib.util.spec_from_file_location("assetstudio_bridge", source)
bridge = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = bridge
spec.loader.exec_module(bridge)
assert not any("rust" in addon.module.lower() for addon in bpy.context.preferences.addons)
original_objects = set(bpy.data.objects)
original_selection = set(bpy.context.selected_objects)
original_active = bpy.context.view_layer.objects.active
bridge.register()
if os.environ.get("ASSETSTUDIO_BLENDER_TEST_PACKAGE"):
    jobs = bridge.bridge_root() / "jobs"
    jobs.mkdir(parents=True, exist_ok=True)
    job = Path(tempfile.mkdtemp(prefix="validation-", dir=jobs))
    source_package = Path(os.environ["ASSETSTUDIO_BLENDER_TEST_PACKAGE"])
    shutil.copytree(source_package, job / "model")
    fbx_name = next((job / "model").glob("*.fbx")).name
    bridge.atomic_json(job / "request.ready.json", {"version":1,"receiver":bridge.INSTANCE,"models":["model/" + fbx_name]})
deadline = time.time() + 1200
result_path = None
try:
    while time.time() < deadline:
        bridge.tick()
        result_path = next(bridge.bridge_root().glob("jobs/*/result.json"), None)
        if result_path:
            break
        time.sleep(0.2)
    assert result_path, "Timed out waiting for AssetStudio export"
    receipt = json.loads(result_path.read_text(encoding="utf-8"))
    assert not receipt["failed"], receipt
    assert len(receipt["imported"]) == 1, receipt
    assert original_objects.issubset(set(bpy.data.objects))
    assert set(bpy.context.selected_objects) == original_selection
    assert bpy.context.view_layer.objects.active == original_active
    imported = set(bpy.data.objects) - original_objects
    materials = {slot.material for obj in imported for slot in obj.material_slots if slot.material}
    images = set()
    channels = {}
    for material in materials:
        shader = next(node for node in material.node_tree.nodes if node.type == "GROUP")
        assert shader.node_tree.get("assetstudio_shader_version") == 1
        assert shader.outputs["Specular"].is_linked or shader.outputs["Metal"].is_linked
        material_channels = []
        for node in material.node_tree.nodes:
            if node.type != "TEX_IMAGE":
                continue
            images.add(node.image)
            assert Path(bpy.path.abspath(node.image.filepath)).is_file()
            assert node.outputs["Color"].is_linked
            channel = node["assetstudio_channel"]
            material_channels.append(channel)
            if channel in {"normal", "occlusion"}:
                assert node.image.colorspace_settings.name == "Non-Color"
            if channel in {"albedo", "specular", "normal"}:
                assert node.outputs["Alpha"].is_linked
        channels[material.name] = material_channels
    assert materials and images
    # The packaged node is standalone and contains no linked libraries/images.
    assert all(group.library is None for group in bpy.data.node_groups)
    assert all(image.library is None for image in bpy.data.images)
    assert len([g for g in bpy.data.node_groups if g.get("assetstudio_shader_version") == 1]) == 1
    object_count = len(bpy.data.objects)
    bridge.tick()
    assert len(bpy.data.objects) == object_count, "Claimed request was imported twice"
    request = json.loads((result_path.parent / "request.processing.json").read_text(encoding="utf-8"))
    fbx = bridge.package_path(result_path.parent, request["models"][0])
    # Reimport with colliding material names must still map every material.
    repeated = bridge.import_package(fbx)
    assert repeated["materials"] == len(materials)
    assert len([g for g in bpy.data.node_groups if g.get("assetstudio_shader_version") == 1]) == 1
    before_failure = set(bpy.data.objects)
    original_build = bridge.build_material

    def fail_build(*args):
        raise RuntimeError("intentional rollback probe")

    bridge.build_material = fail_build
    try:
        bridge.import_package(fbx)
        raise AssertionError("Failed material import unexpectedly succeeded")
    except RuntimeError as error:
        assert "rollback probe" in str(error)
    finally:
        bridge.build_material = original_build
    assert set(bpy.data.objects) == before_failure
    try:
        bridge.package_path(result_path.parent, "../escape.fbx")
        raise AssertionError("Path traversal accepted")
    except (ValueError, FileNotFoundError):
        pass
    summary = {"receipt": receipt, "meshes": sum(obj.type == "MESH" for obj in imported),
               "vertices": sum(len(obj.data.vertices) for obj in imported if obj.type == "MESH"),
               "armatures": sum(obj.type == "ARMATURE" for obj in imported), "materials": len(materials),
               "images": len(images), "channels": channels, "repeat_import": repeated,
               "original_scene_preserved": True, "rollback_verified": True, "rust_extension_enabled": False}
    evidence = ROOT / "target" / "acceptance" / "blender-bridge" / "blender-evidence.json"
    evidence.parent.mkdir(parents=True, exist_ok=True)
    evidence.write_text(json.dumps(summary, indent=2), encoding="utf-8")
    print("BRIDGE_VERIFIED", json.dumps(summary))
finally:
    bridge.unregister()
