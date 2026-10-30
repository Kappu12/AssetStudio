"""Install the built bridge ZIP using Blender's native extension manager.

Run in background factory Blender. Pass -- --user to preserve and update the
existing saved user preferences; without it, installs into the test profile.
"""
import hashlib
import json
from pathlib import Path
import shutil
import sys
import time

import bpy

ROOT = Path(__file__).resolve().parents[1]
user = "--user" in sys.argv
if user:
    preferences = Path(bpy.utils.user_resource("CONFIG")) / "userpref.blend"
    if preferences.is_file():
        backup = ROOT / "target" / "acceptance" / "blender-bridge" / f"userpref-before-{time.time_ns()}.blend"
        backup.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(preferences, backup)
        print("PREFERENCES_BACKUP", str(backup), hashlib.sha256(backup.read_bytes()).hexdigest())
    bpy.ops.wm.read_userpref()

before = {addon.module for addon in bpy.context.preferences.addons}
repos = list(bpy.context.preferences.extensions.repos)
repo = next((r for r in repos if r.module == "user_default"), None)
if repo is None:
    raise RuntimeError("Blender's user_default extension repository is unavailable")
result = bpy.ops.extensions.package_install_files(
    filepath=str(ROOT / "Build" / "assetstudio_bridge.zip"),
    repo=repo.module, enable_on_install=True,
)
if result != {'FINISHED'}:
    raise RuntimeError(f"Extension installation failed: {result}")
module = "bl_ext.user_default.assetstudio_bridge"
after = {addon.module for addon in bpy.context.preferences.addons}
assert before.issubset(after), "Installation unexpectedly removed an existing addon preference"
assert module in after, "The bridge was not enabled"
extension = sys.modules[module]
group = extension.rust_group()
assert group.library is None
inputs = {socket.name for socket in group.interface.items_tree if socket.item_type == 'SOCKET' and socket.in_out == 'INPUT'}
assert {"Albedo Map", "Specular Map", "Nmap", "Unity Nmap", "AO"}.issubset(inputs)
bpy.ops.wm.save_userpref()
print("BRIDGE_INSTALLED", json.dumps({"module": module, "directory": str(Path(repo.directory) / "assetstudio_bridge"),
      "preserved_addons": len(before), "added_addons": sorted(after - before), "shader": group.name}))
