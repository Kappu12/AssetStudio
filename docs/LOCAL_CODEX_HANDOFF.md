# Standalone Rust Blender bridge — 2026-10-30 Windows

New top-toolbar Export to Blender sends checked hierarchy roots (preferred) or
selected GameObjects/Animators. GUI worker exports split FBX packages and waits
for the Blender receipt, updating the existing progress footer. Transport is a
local atomic JSON file queue under LOCALAPPDATA/AssetStudio/BlenderBridge. An
environment override isolates acceptance tests. Packages remain there because
imported image paths reference them. Canonical Windows output paths must also
be used when stripping the job prefix; otherwise strip_prefix fails.

blender_extension/assetstudio_bridge is a separately GPL-3.0-or-later extension.
It bundles an editable copy of Rust Toolset 3.3.0 by llennoco's Rust shader and
two nested node groups, renamed AssetStudio Rust, saved in Blender 5.3. No game
images/geometry, Rust Toolset/Ruster modules or linked shader libraries ship.
Runtime imports only bpy and Python's standard library. The extension uses its
own node copy even if a user group named Rust is present. The native extension
manager validated and installed/enabled it in the user's Blender 5.3 profile,
preserving all 15 previous addon preferences; backup/evidence lives in ignored
target/acceptance/blender-bridge. Blender was closed during installation.

Core blender_materials builds one catalog texture-name index per batch and
emits original Unity channels/UV transforms/floats/colors into the sidecar.
Stable FBX material IDs prevent same-name collisions. Direct references win;
missing companions use exact material/referenced-map stems plus known Rust
suffixes (including workshop _MainTex_<id> names). Unique same-file candidates
win over other bundles. Ambiguous matches are reported and left unassigned.
Synthetic cross-file recovery and duplicate rejection tests pass. The copied
node gets specular/metallic output, Unity packed normals and alpha, sRGB/data
colour spaces, UV transforms, normal strength and occlusion strength.

Factory Blender real DragonLauncher proof: 23 imported objects, ten meshes,
48,072 vertices, one armature, eight materials, 29 texture nodes. Repeat imports
retain all material mappings; claimed requests cannot import twice. Fault
injection verifies object rollback, original objects/selection remain intact,
and Rust extensions are disabled. See scripts/validate_blender_bridge.py and
target/acceptance/blender-bridge/blender-evidence.json. Native ZIP installation
is also tested in an isolated profile. Full Rust suite: 1,032 passed/17 ignored;
strict Clippy and Python release checks pass. Release packaging now includes
assetstudio_bridge.zip and docs/blender-bridge.md.

# Model navigation and checkbox exports — earlier checkpoint

The user's screenshot showed clicks intercepted by selectable table-cell text.
Row child UIs now disable text selection; pointer regressions click all four
columns at their text centers and verify row selection with no text cursor.

Right-click Mesh/GameObject/Animator -> Show in scene hierarchy resolves actual
GameObject owners. Mesh ownership is indexed lazily, bounded and cached, with
orphan/superseded filters excluded. Multiple owners have a Next owner control.
Model root jumps to the enclosing top-level GameObject. SceneTree keeps virtual
DFS rows, parent/subtree intervals, check state and collapsed branches. Checking
a parent includes its descendants; highest checked roots become separate model
packages. Reveal opens ancestors and scrolls to the target. Checkboxes and row
preview selection remain independent. Disclosure icons are painted vectors.

The actual DragonLauncher_body mesh found two owners in the supplied collection.
Its viewmodel exposed populated AnimatorController behaviour-tail metadata;
the reader now retains state/layer ranges, uint indices, MonoBehaviour PPtrs and
threading state with existing count/reference/allocation bounds. Both-endian
truncation and malformed-range/index regressions pass. Default animation export
remains enabled. Package names use a bounded prefab leaf plus object identity,
avoiding importer path limits caused by repeating full asset paths.

Real workflow: full folder -> mesh owner -> Model root -> checked subtree ->
binary FBX package. Blender 5.3 independently imported 10 meshes, 48,072 vertices,
75,471 polygons, eight materials, 25 file images, one armature and 20 actions;
no missing images. Artifacts remain ignored under target/acceptance/dragon-*. The
latest successful package is identified by its run's navigation-evidence.json;
latest-blender-import.json points to the independently imported FBX. Native UI
events are covered by headless pointer tests; no OS/GPU interaction is claimed.
Full workspace: 1,029 tests pass, 17 optional tests ignored; real workflow probe
also passes explicitly. Strict Clippy and formatting pass. Prior checkpoints follow.

# Folder loading follow-up — 2026-10-30 Windows

The user requested faster actual folder loading. A fresh full-corpus baseline
was 23.995 seconds to load plus 3.655 seconds for browser indexes. File discovery,
serialized metadata and reference indexing used less than one second; object names
took 16.206 seconds, name resolution 2.231 and catalog construction 4.657.

StudioObject::metadata combines name/container lookup. Short bounded_name inputs
return before Unicode scanning. The browser shares Session's immutable Arc identity
map rather than rebuilding it. Name readers explicitly request per-file read-ahead
with a bounded adaptive 4–64 KiB window; ordinary hierarchy/payload reads bypass
that cache. Read-ahead is scan-specific to avoid overfetching random model data.
The final scan-specific cache loads in 10.974 seconds plus 2.899 for browser
indexes (13.873 seconds until browsing, versus 27.651 before). Name scanning takes
4.941 seconds; the complete hierarchy still reads in 17.220 seconds plus 0.458 for
row ordering. See folder-load-final-scoped.log for the actual optimized run.
Tests include independent concurrent readers, neighboring objects, sparse prefixes,
bulk bypass, and file/region bounds. Full workspace: 1,019 tests passed, 16 ignored.

Baseline/final profiles and the streaming full-listing SHA-256 comparisons live
under ignored target/acceptance/folder-*. The complete JSON listing is 1,769,555,850
bytes; compare folder-catalog-before.json and folder-catalog-after.json for exact
record equivalence. Keep builds in Build with the existing native launcher aliases.
No push is authorized. Earlier checkpoints follow.

# Desktop type filters and performance — 2026-10-30 Windows

The desktop has a #090909/white theme and persistent worker progress footer.
Types now support independent checkboxes, case-insensitive search and an A/↓/↑
cycle for alphabetical/most/fewest ordering. All types clears the checked filters.
Search and sorting the type list preserve checked types. Headless pointer-input
tests exercise the actual checkbox and sort controls.

Session metadata is shared with the browser through immutable Arc snapshots;
metadata recovery uses copy on write. Name ordering and per-type rank buckets
avoid repeated full catalog sorts. Pending filters retain the displayed rows.
Ordinary asset selection avoids constructing millions of ordered keys. Hierarchy
row ordering uses indexed adjacency and iterative traversal. The browser's explicit
topology reader skips model payloads, retaining graph budgets and cycle checks;
Session::scene and model/export readers retain their original full validation.

Bounded read-ahead cursors cache metadata/container/name/scene fields across
logical seeks, reducing Windows file calls. Scalar/bulk/boundary/EOF regressions
pass. Full workspace: 1,017 tests passed, 16 ignored (14 external and two optional
local performance probes); strict Clippy, formatting and release build pass.

Real corpus probe logs are ignored under target/acceptance/browser-*.log. The
final 6,591,789-asset load took about 22 seconds, index preparation four seconds,
all rows 6.7 ms, Mesh 0.24 ms, Mesh+Texture2D 1.7 ms. All indexed rows matched the
reference. Content hierarchy reading improved from 37.5 seconds to 3.8–5.1 seconds.
No remote push is authorized. Earlier checkpoints follow.

# Facepunch Rust checkpoint — 2026-10-30 Windows

The user chose **finish the Facepunch Rust workflow first**; unrelated game-profile
ports are deferred. Work remains local on rust-rewrite. No push or release is
authorized. Prior checkpoint commits: 550e0e7 (Windows fixes), afa4382 (legacy
ACL/BH3Pre and configurable catalog capacity).

The full supplied corpus loads: 6,591,789 assets, no load diagnostics. Real WAV,
PNG and OBJ samples validate. The complete textured camper FBX exports 17 files
without failures and imports into Blender 5.3: 78 meshes, 34,188 vertices, 30,374
polygons, three materials and ten file images, no missing images.

Terminal-only streams, minimum-finite sentinels and discrete binding offsets are
fixed. Property-name recovery scans each serialized type once. Selected model
preview/export reads bounded subtrees and ancestor/bone/static-batch dependencies,
so unrelated collection size or broken renderers do not prevent selection export.
The desktop defaults to a ten-million catalog capacity; the CLI retains one
million unless --max-assets is set. Full hierarchy display follows that budget.

Final checks: 1,009 Windows Rust tests passed, 14 external checks ignored; 35
Python checks passed, two symlink tests unavailable; strict Clippy, formatting,
policy and optimized MSVC GUI/CLI build passed. Interactive computer-control
approval timed out; headless native smoke checks passed. Private data/outputs
remain ignored under target/acceptance. See docs/facepunch-rust.md.

Standalone .anim non-null references still need Unity GUID metadata; unbound
clip paths need their hierarchy/Avatar. Ten specialized profiles remain rejected.
These changes do not claim complete rewrite parity. Earlier handoffs follow.


## Build / packaging

Native MSVC release output: target/windows-release-validation/release. Portable
ZIP generation and extracted smoke checks pass. Exact packaging/license bytes
are protected with -text in .gitattributes to preserve provenance hashes on
Windows. Publish the ZIP contents into Build and retain the existing
Build/AssetStudio.GUI.exe and Build/AssetStudio.CLI.exe entry points as native
aliases. A:\Blender\AssetStudio.bat already points to the GUI compatibility path.
Keep previous launcher binaries as .pre-rust.bak. Do not put builds under A:\Blender.

## Remaining boundaries

Standalone .anim files still need actual Unity GUID metadata for non-null
object/script references; paths require bound hierarchy/Avatar data. Synthetic
and real sampled checks are recorded separately in feature-parity.md. GUI GPU/OS
interaction was not inspected because desktop-control approval timed out; native
smoke checks passed. Full 39-profile parity is not claimed.

The preserved ACL 1.1 work is now registered and supports verified variable-QVV
4/3/3 clips through the BH3Pre adapter. Source research and original DLL/C++
oracles remain in vendor/unity-rs-core/tests/fixtures/acl-legacy and
crates/assetstudio-core/tests/fixtures/animation-mhy. SR/database/full/fixed
selectors remain separate contracts.
