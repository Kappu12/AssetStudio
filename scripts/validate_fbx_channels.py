#!/usr/bin/env python3
"""Independently check optional FBX writer samples using ufbx 0.0.5.

Install the test-only reader with `python -m pip install ufbx==0.0.5`, generate
samples by running the `explicit_settings_keep_all_vertex_channels` Rust test
with ASSETSTUDIO_FBX_WRITER_TEST_OUT set, then pass that directory here.
Nothing is uploaded and no sample becomes a release asset.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import traceback

VERSIONS = (7100, 7200, 7300, 7400, 7500)
READER_LIFETIMES: list[object] = []


def check_one(path: Path) -> None:
    from importlib.metadata import version
    import ufbx

    if version("ufbx") != "0.0.5":
        raise ValueError("The recorded independent reader is ufbx 0.0.5")
    scene = ufbx.load_file(str(path))
    # Keep borrowed wrappers alive until the disposable process exits. The
    # wheel can invalidate them when temporary list elements are collected.
    nodes = list(scene.nodes)
    materials = list(scene.materials)
    bones = list(scene.bones)
    textures = list(scene.textures)
    READER_LIFETIMES.extend((scene, nodes, materials, bones, textures))
    if path.name.startswith("texture-transform-"):
        assert scene.metadata.version == 7400
        assert len(scene.meshes) == 2 and len(materials) == 2 and len(textures) == 2
        expected = {"skin": ((0.0, 0.0, 0.0), (1.0, 1.0, 1.0)),
                    "other": ((0.25, 0.5, 0.0), (2.0, 3.0, 1.0))}
        for material in materials:
            bindings = list(material.textures)
            READER_LIFETIMES.append(bindings)
            binding = next(binding for binding in bindings if binding.material_prop == "DiffuseColor")
            texture = binding.texture
            transform = texture.uv_transform
            READER_LIFETIMES.extend((texture, transform))
            assert texture.name == "Body.png" and texture.uv_set == "UV0"
            translation = transform.translation
            scale = transform.scale
            actual = ((translation.x, translation.y, translation.z), (scale.x, scale.y, scale.z))
            assert actual == expected[material.name], (material.name, actual)
        print(json.dumps({"file": path.name, "meshes": 2, "materials": 2,
                          "shared_image_files": 1, "distinct_texture_transforms": 2}), flush=True)
        os._exit(0)
    if path.name.startswith("uv-selection-"):
        channel = path.name.split("-")[2]
        expected = "" if channel == "none" else f"UV{channel}"
        assert scene.metadata.version == 7400
        assert len(textures) == 1 and textures[0].uv_set == expected
        meshes = list(scene.meshes)
        READER_LIFETIMES.append(meshes)
        for mesh in meshes:
            names = [uv.name for uv in mesh.uv_sets]
            assert (not names) if channel == "none" else expected in names
        print(json.dumps({"file": path.name, "meshes": len(meshes), "texture_uv_set": expected}), flush=True)
        os._exit(0)
    expected_version = int(path.name.split("-")[1])
    assert scene.metadata.version == expected_version
    assert len(scene.meshes) == 1 and len(scene.materials) == 1
    mesh = scene.meshes[0]
    assert mesh.name == "quad"
    assert materials[0].name == "skin"
    assert sorted(node.name for node in nodes if not node.is_root) == ["TransformOnly", "root"]
    assert mesh.num_vertices == 3 and mesh.num_faces == 1
    assert list(mesh.vertex_indices) == [2, 1, 0]
    first = mesh.vertices[0]
    assert (first.x, first.y, first.z) == (-0.5, 0.0, 0.0)
    assert mesh.vertex_normal.exists and mesh.vertex_tangent.exists and mesh.vertex_color.exists
    normal = mesh.vertex_normal.values[0]
    assert (normal.x, normal.y, normal.z) == (0.0, 0.0, 1.0)
    tangent = mesh.vertex_tangent.values[0]
    assert (tangent.x, tangent.y, tangent.z) == (-1.0, 0.0, 0.0)
    color = mesh.vertex_color.values[1]
    assert (color.x, color.y, color.z, color.w) == (0.0, 1.0, 0.0, 0.5)
    assert [channel.name for channel in mesh.uv_sets] == [f"UV{i}" for i in range(8)]
    for index in range(8):
        uv = mesh.uv_sets[index].vertex_uv.values[0]
        assert (uv.x, uv.y) == (float(index), 0.0)
    assert list(mesh.face_material) == [0]
    assert len(scene.bones) == 1
    assert bones[0].name == "TransformOnly"
    assert bones[0].props.find_real("Size", -1.0) == 37.5
    assert len(scene.textures) == 8
    assert all(texture.name == "Body.png" for texture in textures)
    connections = {binding.material_prop: binding.texture.uv_set for binding in materials[0].textures}
    for index, name in enumerate(("DiffuseColor", "NormalMap", "SpecularColor", "Bump", "AmbientColor", "EmissiveColor", "ReflectionColor", "DisplacementColor")):
        assert connections[name] == f"UV{index}"
    print(json.dumps({"file": path.name, "version": expected_version, "vertices": 3, "uv_sets": 8, "material_channels": 8, "bone_size": 37.5}), flush=True)
    # This wheel's borrowed-wrapper destructors can crash at interpreter
    # teardown. All checks run first; one disposable subprocess owns each scene
    # and exits without invoking those third-party cleanup wrappers.
    os._exit(0)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--one", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.one:
        try:
            check_one(args.directory)
        except BaseException:
            traceback.print_exc()
            sys.stderr.flush()
            os._exit(1)
    for version in VERSIONS:
        for kind in ("ascii", "binary"):
            sample = args.directory / f"channels-{version}-{kind}.fbx"
            subprocess.run([sys.executable, str(Path(__file__).resolve()), str(sample), "--one"], check=True)
    for sample in sorted(args.directory.glob("uv-selection-*.fbx")):
        subprocess.run([sys.executable, str(Path(__file__).resolve()), str(sample), "--one"], check=True)
    for sample in sorted(args.directory.glob("texture-transform-*.fbx")):
        subprocess.run([sys.executable, str(Path(__file__).resolve()), str(sample), "--one"], check=True)


if __name__ == "__main__":
    main()
