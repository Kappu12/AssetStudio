# AssetStudio compatibility patches

This source is copied from the MIT-licensed `unity-rs-core` 0.6.1 at
`https://github.com/seiunx-dev/unity-rs`, commit
`b3e509ed09fe6ff3a897d04ddb75eb5214482265`.
The original licenses and notices are retained unchanged. It is an existing
independent Rust format reader, not newly authored application code.

The standalone manifest resolves the upstream workspace package fields without
changing dependency constraints. The parent workspace uses a Cargo patch to make
the exact local source, tests and subsequent compatibility fixes reviewable.
Backend changes must have corresponding tests and be listed below.

## Local changes

- Standalone package metadata and dependency constraints resolved from the
  upstream workspace; the library is an explicit member of the parent workspace
  and shares its lockfile. Workspace check, Clippy and test commands include it.
- Explicit `GameVariant` selection carried through root/nested container loading
  and SerializedFile metadata. TOT/BH3PrePre preserve their player layout when
  the declared platform is `NoTarget`; other declared platforms are unchanged.
  Naraka, Ex Astris and Love and Deepspace have their documented material,
  texture, mesh, shader, controller and animation field variants. This does not
  infer a game from bytes or make opaque animation codecs interchangeable.
  Ex Astris resident dense quantization decodes 8/16/24/32-bit samples;
  nonzero generic quantization groups remain unsupported. Love and Deepspace
  uniform packed morph records are validated/converted; mixed/variable record
  forms fail explicitly. Secondary shader storage is retained and bounded;
  text conversion still uses the standard primary program blob.
- Static-batch renderer subset/root metadata retained in scene/model bindings.
  Bounded matrix reconstruction and vertex compaction recover each renderer's
  local geometry from world-space or batch-root-space storage, retaining
  normals, tangents, colors and UV0–7. Model lookup indexes can be rebuilt after
  application-side filtering/scaling. Unsupported nonempty mesh failures now
  propagate rather than silently dropping geometry from mixed scenes.
- ASCII/binary FBX settings for versions 7100–7500, transform-only joint casting,
  bone display size and UV semantics. Binary 7500 records use 64-bit headers;
  older versions use 32-bit headers. Both scene writers retain normal, UV,
  color, tangent and polygon-material layers. Binary object names use the
  format's NUL/SOH class suffix, matching ASCII names after import.
- Texture bindings select a UV set actually emitted by every geometry using
  their material, omit a named UV reference when no UVs exist, and reject
  incompatible shared-material coordinate layouts. Distinct UV sets or finite
  offset/scale values receive distinct Texture objects while image files remain
  shared. The eight explicit material texture slots are also available to MTL.
- FBX global transforms use bounded iterative traversal, including valid deep
  child-before-parent graphs, with cycle checks instead of call-stack recursion.
- Safe native decoding API `acl::decode_compressed_tracks` for resident standard
  ACL scalar tracks (release format versions 7/10) and ACL 2.1 QVV tracks
  (version 10). Independent ACL 2.1 compressor/decompressor fixtures cover
  scalar and QVV sample values, segmentation, quantization, constants, raw
  fallback and wrap optimization, including a durable 96-case edge corpus with
  123,300 sample comparisons. This API does not supply Unity-fork decoder
  maps, external streams, stripped keyframes or external default poses.
- Rust 1.99 Clippy compatibility cleanups in inherited code, without a blanket
  warning suppression. Targeted malformed-input, resource-budget and feature
  regressions accompany the compatibility changes.
- Upstream corpus manifest and optional .NET oracle project retained at usable
  vendored test paths. Optional external oracle tests remain explicitly ignored
  unless the required external tools/corpus are supplied.

The application-side game wrappers and modern `.anim` expansion live in
`crates/assetstudio-core`; they are not upstream backend changes. The optional
`scripts/validate_fbx_channels.py` uses ufbx only as a development reader and
does not become an application dependency. ACL's C++ oracle generator and the
managed game-format oracles are development fixtures; Cargo does not compile
or execute them. Their provenance and retained licenses are beside the fixtures.

Release dependency inventory records the upstream revision and a SHA-256 digest
of this manifest and every file under `src/`, including binary decoder tables.
That digest identifies the locally modified reader actually selected by Cargo;
the upstream revision alone does not identify the patched build. This change
record and the original legal/notice bundle are included in release ZIPs.

## Arknights Endfield follow-up

Explicit Endfield selection retains its material UV index, renderer flags, mesh
collision fields/metric, controller fields, and post-WrapMode animation mode and
quantized dense ordering. Both-endian fixtures check truncations and budgets.
Collision-baked meshes fail triangle export explicitly, matching the baseline
which skips triangle conversion for that layout. App wrappers have original C#
FairGuard/LZ4Inv oracles and real Session material/animation export checks.

The ACL 1.1 variable-QVV decoder is registered in `acl`, including its original
DLL/upstream C++ oracle and malformed-input tests. BH3Pre reads the original MHY
byte-array/curve-count layout and feeds contiguous R4/T3/S3 samples into `.anim`
and model conversion before standard curves. Root hash-zero model tracks resolve
to their animation owner. Seed-keyed bundle headers, pack-key blocks and the
half-resolution renderer flag are supported by the application adapter. Full/fixed
selectors, SR and database formats remain unsupported. Unix-only extraction test
imports are gated for strict Windows linting.

## Facepunch Rust collection and animation fixes

Selected hierarchy reading follows only requested GameObject descendants plus
ancestors, skin-bone owners and static-batch roots. Pending traversal, node,
component/reference and index-byte limits remain bounded; cycles and corrupt
selected components remain errors. The application uses this path for model
preview, selected export and model animation graphs. Model animation dense and
constant offsets include discrete streamed curves. Application-side `.anim`
expansion additionally accepts terminal-only empty streams and minimum-finite
slope sentinels, validates discrete step coefficients and deduplicates property
tree scans by serialized type. See the root feature table for real-corpus and
independent Blender import evidence.

Windows checkout attributes preserve exact vendored legal/provenance bytes so
real dependency-license SHA-256 checks survive core.autocrlf. The actual portable
Windows ZIP, its PE imports, source/license inventory and extracted native smoke
checks passed locally after that correction.
