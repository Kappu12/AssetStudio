# Review before merging the Rust rewrite into main

Reviewed the native application and recent Facepunch Rust workflow against main
at `94318fa`, including worker/result handling, shared catalog indexes, hierarchy
selection, output/source protection, model packages, the Blender queue/material
handoff, and release packaging. Earlier reader/format investigations remain in
the [review records](../review-sweeps.md). The legacy managed implementation is
retained as a reference and is not built by the Cargo release workflow.

## Findings fixed

- **Receiver selection:** discovery previously chose the latest heartbeat twice.
  Another Blender window could win the second lookup and reject an otherwise
  valid export. New exports choose the latest started receiver, then check that
  exact receiver throughout export. Fresh heartbeat/filename validation remains.
- **Queue failures:** an unsupported protocol or invalid models payload could
  remain ready forever and block later work. Owned requests are claimed before
  validation and receive a terminal failure. Malformed JSON also gets claimed;
  another receiver's valid requests remain untouched. Existing successful
  receipts are not overwritten by a later operator error.
- **False success:** malformed or empty Blender receipts could report success
  despite acknowledging fewer models than requested. Receipt fields are now
  typed and successful imports must acknowledge every requested model.
- **Rollback leaks:** failed imports removed objects but left their mesh, rig,
  action and other datablocks. Cleanup now removes the entire newly created
  graph in one batch while preserving every pre-existing datablock.
- **Texture interpretation:** competing specular/metallic properties could keep
  the first image but choose the last property's shader workflow. Workflow now
  follows the actual chosen binding. Recovery also prioritizes the referenced
  albedo stem over a generic material name, and ignores unknown properties as
  recovery hints.
- **Recovery budget:** companion-map recovery now shares the initial model's
  encoded-texture budget instead of growing to the whole batch's output limit.

The extension is tracked under `blender_extension/assetstudio_bridge`, including
its source, manifest, copied editable shader and GPL license/provenance. Release
packages contain its installable ZIP. The README now links to the source and
explains how to package and use it from the main checkout.

## Validation

Local Windows checks cover the complete Cargo workspace, strict Clippy,
formatting, Python release/queue regressions, workflow policy and the native
release build. Regression cases cover competing material workflows, cross-file
recovery and ambiguity, multiple receivers, invalid/incomplete receipts, malformed
and foreign requests, and existing paths outside a package.

The full Rust suite passed 1,035 tests with 17 optional/local probes ignored.
The Python suite passed 40 tests with two platform-dependent cases skipped.

Factory Blender 5.3 imports the actual DragonLauncher package with ten meshes,
48,072 vertices, one armature, eight mapped materials and 29 texture nodes.
Repeat imports preserve mappings and the existing scene/selection. Injected
material failures now verify exact preservation of all previous objects,
meshes, armatures, materials, images, actions, collections, node groups and
libraries. Rust Toolset and Ruster are disabled during this check.

Linux/macOS execution and the optional external/private corpus suites were not
run locally. The configured hosted matrix covers all three operating systems.
Existing documented game/profile and animation limits remain; this merge does
not claim complete compatibility with every Unity game.
