The bundled `rust_shader.blend` contains the Rust shader node and its two
dependent node groups, copied from **Rust Toolset 3.3.0 by llennoco** as installed
by the user. Rust Toolset declares GNU GPL version 3 or later. This extension
and its shader asset are distributed under the same license; see LICENSE.txt.

The original top-level `Rust` group was renamed `AssetStudio Rust` and saved
with its nested `Specular Cycles` and `XY Normalmap` groups using Blender 5.3.
No game geometry, textures, Rust Toolset operators, Ruster modules, or external
library links are included. The editable node source is the bundled .blend.

The bridge imports this local copy directly. It never locates, enables, or
imports Rust Toolset or Ruster at runtime. AssetStudio and this separately
licensed Blender extension exchange model files and JSON through a local queue.
