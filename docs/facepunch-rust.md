# Facepunch Rust workflow

Use the **Normal** game profile. Facepunch Rust assets use the ordinary Unity
reader; the `rust-rewrite` branch name refers to the implementation language.

## Desktop

The native Rust desktop is published to `Build/AssetStudio.exe`. The compatibility
entry point `Build/AssetStudio.GUI.exe` runs the same native binary, so the existing
`A:\Blender\AssetStudio.bat` launcher keeps working. `Build/AssetStudioCLI.exe`
is the native CLI; `Build/AssetStudio.CLI.exe` is its compatibility entry point.
Previous launcher executables are retained as `.pre-rust.bak` files.

The desktop uses a `#090909` background with white controls. After loading, select
**Mesh**, **Texture2D**, **AudioClip**, or multiple checkboxes under **Types** in the
left sidebar. Checked types are combined; **All types** clears those selections.
**Search types** searches the type names without changing the checked types.
The button next to **Types** cycles **A** (alphabetical), **↓** (most assets first)
and **↑** (fewest assets first). Ties remain alphabetical. The asset-name search
box above the list further narrows the matching assets.
Use **GameObject** or **Animator** for complete models rather than individual meshes.
Additional formats and grouping controls are under **Export options**.

The bottom progress bar stays visible during background operations. Opening assets
reports file, asset-name, catalog and browser preparation counts; asset exports
report processed items. Percentages apply to the current named stage. Stages with
no known total show an animated bar and elapsed time.

Browser metadata shares the loaded session instead of copying every asset string.
Name ordering and type buckets are built once; changing type or name filters reuses
them. Existing rows remain visible while a new filter runs. The hierarchy browser
reads GameObjects and Transforms without decoding renderer/material/skin payloads;
model previews and exports retain the full model readers and their validation.
Bounded read-ahead buffers reduce small Windows file reads during metadata,
container, name and scene parsing.

Local optimized Windows checks on this corpus restored 6,591,789 visible rows in
about 7 ms, filtered 56,733 meshes in about 0.24 ms, and combined Mesh/Texture2D in
about 1.7 ms; indexed results matched the reference filter exactly. Initial loading
takes about 11 seconds plus three seconds to prepare browser indexes after the
folder-loading follow-up, compared with about 24 seconds plus four previously.
On the content bundle sample, reading 245,936 hierarchy nodes fell from 37.5 seconds to about
3.8–5.1 seconds. Timings depend on storage, file caching and available memory.

Folder name scans reuse an adaptive file read-ahead window, growing from 4 KiB
to at most 64 KiB for nearby reads. This is enabled explicitly for name scanning;
hierarchy, payload and other random reads keep their normal access path. The
browser shares the session's validated identity lookup, and catalog construction
retrieves name/container metadata together and avoids scanning short strings
solely to apply their length cap.

Open the complete `A:\Blender\Rust_Assets` folder to resolve models' external
texture CABs. Opening only `content.bundle_unpacked` provides geometry but leaves
the external texture bundles unavailable. The desktop's catalog capacity defaults
to ten million assets; change it in load settings when needed.

To export a complete model from the asset list:

1. Right-click a **Mesh**, **GameObject**, or **Animator** and choose **Show in scene hierarchy**.
2. Use **Model root** to jump to the containing top-level model. If a mesh is reused,
   **Next owner** visits its other GameObjects.
3. Tick the desired root's checkbox. Its children are checked too. Branch arrows
   expand/collapse the tree; clicking a row name selects its preview independently.
4. Click **Export checked models (split)** and choose an output folder.

Each highest checked node becomes a separate FBX package; nested checked children
are included in that model rather than exported again. Keep the package together
with its textures when importing into Blender. Export options also control skin,
materials and animations. Package names use the prefab leaf plus a stable object
identity so full asset paths do not become oversized directory/file names.

Selected model preview/export reads that subtree and its required dependencies;
unrelated objects do not consume its preview node limit. Mesh-to-GameObject lookup
is prepared lazily on the worker and reused for subsequent navigation, preserving
the folder-loading path.

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

The Mesh-to-hierarchy checkbox workflow also exports `DragonLauncher_body` through
its complete viewmodel root. Blender independently imports ten meshes, one armature,
20 animation actions and 25 loaded file images with no missing textures. Populated
AnimatorController behaviour metadata is retained rather than rejected.

Standalone Unity `.anim` files containing non-null object/script references still
require Unity GUID metadata. Raw/JSON inspection remains available. This workflow
does not enable the ten deferred specialized game profiles; see
[feature parity](feature-parity.md) and [game compatibility](game-compatibility.md).
