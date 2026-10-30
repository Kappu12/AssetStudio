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
