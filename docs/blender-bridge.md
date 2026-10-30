# AssetStudio Rust Bridge

This standalone Blender extension targets **Facepunch Rust** and Blender 5.3
or newer. It includes a copy of Rust Toolset's Rust material node and does not
need Rust Toolset or Ruster installed or enabled.

1. In Blender Preferences → Get Extensions → menu → Install from Disk, choose
   `assetstudio_bridge.zip` from AssetStudio's Build/release folder and enable it.
2. Open the Rust asset folder in AssetStudio. Keep Blender in Object Mode.
3. Tick model roots in Scene hierarchy, or select GameObjects/Animators in the
   asset list, then click **Export to Blender** in the top toolbar. A Mesh can
   be located with right-click → Show in scene hierarchy → Model root.
4. The progress bar covers export and Blender import. Models appear in separate
   `AssetStudio · …` collections. Existing scene objects and selection remain.

The bridge sends binary FBX, decoded PNG textures, and a material sidecar.
It preserves skins, animations, and model options while always exporting
materials and textures. Unity property names choose albedo, specular/metallic,
normal, emission, and occlusion connections, including packed alpha channels,
colour spaces, UV scales/offsets, normal strength and the Unity normal switch.
The shader has both specular and metallic outputs.

Missing channels are searched across **all loaded Texture2D assets**, including
other bundles. Searches use the material name and the stems of referenced
textures with Rust Toolset's channel suffix conventions. Direct Unity references
take priority. A unique same-file match takes priority over other bundles;
ambiguous matches are left unresolved and reported. Load the complete game asset
folder to make those other bundles available. This cannot recover a texture that
was never loaded or infer an arbitrary unrelated texture name.

The extension's AssetStudio sidebar shows connection/import status. Diagnostics,
unresolved channels and import receipts are also retained with each package under
`%LOCALAPPDATA%/AssetStudio/BlenderBridge/jobs`. Keep these packages while the
imported .blend uses their textures, or pack Blender's external resources before
moving it. Exports are retained deliberately; source game files are read only.
When several Blender instances are open, a new export selects the most recently
started receiver and keeps that receiver for the entire export. Close the other
instances to target a particular file reliably.

The queue uses local files only; there is no listening network port or required
Python package. Requests are claimed before import to prevent duplicate imports
after reload. Import failures roll back newly imported objects for that model.
Reloading a .blend keeps the bridge running. Disabling the extension stops it.

Developers can set `ASSETSTUDIO_BLENDER_BRIDGE` in both applications to isolate
the queue, then run `scripts/validate_blender_bridge.py` inside factory Blender
to verify the copied shader, material wiring, repeat imports and queue receipts.
`python scripts/package_blender_extension.py --output Build/assetstudio_bridge.zip`
builds an installable ZIP without including caches or test data.
