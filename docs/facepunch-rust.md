# Facepunch Rust workflow

Use the **Normal** game profile. Facepunch Rust assets use the ordinary Unity
reader; the `rust-rewrite` branch name refers to the implementation language.

## Desktop

The native Rust desktop is published to `Build/AssetStudio.exe`. The compatibility
entry point `Build/AssetStudio.GUI.exe` runs the same native binary, so the existing
`A:\Blender\AssetStudio.bat` launcher keeps working. `Build/AssetStudioCLI.exe`
is the native CLI; `Build/AssetStudio.CLI.exe` is its compatibility entry point.
Previous launcher executables are retained as `.pre-rust.bak` files.

Open the complete `A:\Blender\Rust_Assets` folder to resolve models' external
texture CABs. Opening only `content.bundle_unpacked` provides geometry but leaves
the external texture bundles unavailable. The desktop's catalog capacity defaults
to ten million assets; change it in load settings when needed.

Filter GameObjects/Animators, select the desired model and export binary FBX.
Selected model preview/export reads that subtree and its required dependencies;
unrelated objects do not consume its preview node limit. Keep the exported model
folder together with its textures when importing into Blender.

## CLI

The CLI defaults to a one-million-asset catalog. Use the explicit larger limit
for this complete extracted corpus:

```powershell
Build\AssetStudioCLI.exe A:\Blender\Rust_Assets --operation inspect --max-assets 10000000 --json
Build\AssetStudioCLI.exe A:\Blender\Rust_Assets exported --operation model --max-assets 10000000 --types GameObject --names '^camper_module$' --model-format fbx-binary
```

Existing outputs are preserved. Retain `.resS`/`.resource` files and original CAB
names. Load the containing collection when an export reports an unresolved
external file, hierarchy path or Avatar.

## Verified scope

The supplied 53.6 GB corpus loads 6,591,789 assets without load diagnostics.
Real PNG, WAV and OBJ exports validate. The camper FBX exports without failures
and imports into Blender with 78 meshes, 34,188 vertices, 30,374 polygons, three
materials and ten loaded images. These are sampled conversion checks, not an
exhaustive conversion of all 6.6 million objects.

Standalone Unity `.anim` files containing non-null object/script references still
require Unity GUID metadata. Raw/JSON inspection remains available. This workflow
does not enable the ten deferred specialized game profiles; see
[feature parity](feature-parity.md) and [game compatibility](game-compatibility.md).
