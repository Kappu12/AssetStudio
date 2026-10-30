# AssetStudioCLI

The native Rust command-line frontend uses the same sessions, readers, selection,
and exporters as the desktop application.

```sh
AssetStudioCLI game_Data exported --game Normal
AssetStudioCLI bundle --operation list --json
AssetStudioCLI bundle --operation inspect
AssetStudioCLI bundle exported --types Texture2D Sprite --names '^Icon'
AssetStudioCLI bundle exported --export_type Raw --group_assets BySource
AssetStudioCLI bundle exported --write-map --map_type JSON
AssetStudioCLI bundle maps --map_op AssetMap --map_type XML
AssetStudioCLI game_Data maps --map_op AssetMap --map_type MessagePack --path-ids 123 456
AssetStudioCLI game_Data exported --asset-map maps/assets_map.map --asset-base game_Data
AssetStudioCLI game_Data exported --map_op AssetMap,Load --map_name maps/assets_map.json --map_type JSON
AssetStudioCLI bundle models --operation model --model-format fbx-binary
AssetStudioCLI bundle models --operation model --no-animations --no-textures
AssetStudioCLI bundle models --operation model --fbx-version 7500 --scale-factor 0.01 --prune-nodes
AssetStudioCLI bundle models --operation model --uv-channels 0,1 --uv-slot 1:NormalMap
AssetStudioCLI bundle exported --types AnimationClip
AssetStudioCLI archive.split3 exported --game TOT
AssetStudioCLI bundle unpacked --operation extract
AssetStudioCLI game_Data maps --map_op Both --map_type JSON
AssetStudioCLI game_Data/a.bundle exported --cab-map maps/assets_map.bin
AssetStudioCLI game_Data exported --dummy_dlls Game_Data/Managed
AssetStudioCLI game_Data exported --mono-schema schemas.json --export_type JSON
```

`--operation` accepts `export` (the default), `list`, `inspect`, `extract`,
`scene`, `model`, and `map`. The first positional argument is always the input;
the second is the output directory and is required for operations that write files.

Use `--json` for machine-readable list, inspect, and scene output. Those commands
write only their result to stdout; diagnostics go to stderr. Array listings are
streamed rather than assembled as a second complete JSON copy in memory.

## Compatibility

Legacy positional invocations and these option spellings remain available:

| Option | Behavior |
| --- | --- |
| `--types`, `--type` | Class names or numeric IDs; `:Parse`/`:None` entries are kept out of exports, `:Export`/`:Both` are exported. |
| `--names`, `--filter` | Case-insensitive regexes, or one file containing one regex per line. |
| `--containers` | The same filtering on recovered container names. |
| `--path-ids`, `--path_ids` | Restrict selection to exact signed 64-bit PathIDs; combine with class, name, and container filters. |
| `--max-assets` | Catalog capacity from 1 to 10000000 (default 1000000); name, reference-index and text budgets scale with it. Use larger values for large extracted collections. |
| `--game` | Explicit format profile, defaulting to `Normal`; missing game-specific readers produce an error. |
| `--unity_version` | Override the engine version used to decode stripped-version files. |
| `--key_index`, `--keys-file` | Select a UnityCN key by index from the original public key list or a supplied JSON file. |
| `--key`, `--xor_key` | MiHoYoBinData XOR byte, decimal or hexadecimal. |
| `--ai_file`, `--ai-file` | Read an asset-index JSON and recover container names before filtering or exporting. |
| `--group_assets`, `--group_assets_type` | `ByType`, `ByContainer`, `BySource`, or `None`. |
| `--export_type` | `Convert`, `Raw`, `Dump`, or `JSON`. |
| `--map_op AssetMap` | Write only the selected asset map. |
| `--asset-map`, `--map_op AssetMap,Load` | Select exact cached source/PathID/class identities from a JSON, XML, or legacy MessagePack map. The legacy form reads its path from `--map_name` and format from `--map_type`. |
| `--asset-base` | Constrain cached source resolution to this directory; defaults to the input directory or selected input file's parent. |
| `--map_op CABMap`, `Both`, `All` | Write a CAB cache, both maps, or export assets and then write both maps. |
| `--cab-map`, `--map_op CABMap,Load` | Load a dependency cache and resolve its full transitive CAB graph. |
| `--cab-base` | Set the cache generation base, or relocate a previously generated cache. |
| `--mono-schema` | Repeatable trusted JSON MonoBehaviour schemas; a maximum of 64 files. |
| `--dummy_dlls`, `--dummy-dlls` | Recover stripped MonoBehaviour schemas from matching managed DLL metadata using a short-lived Rust helper. |
| `--map_type`, `--map_name` | JSON, XML, or MessagePack (`.map`) and a portable output filename; the output directory comes from the second positional argument. |
| `--no-animations`, `--ignore_controller_anim` | Export models without controller or clip decoding, also during converted asset export. |
| `--no-textures` | Omit model texture decoding and model-side image output. |
| `--no-skins`, `--no-materials`, `--no-blend-shapes` | Omit model weights/bindings, materials and dependent textures, or morph geometry and its tracks. |
| `--prune-nodes` | Keep renderable nodes, required skin bones and their ancestors; remove unused nodes. |
| `--scale-factor` | Scale geometry, bind-pose translations and animation translations; valid range is 0.000001–1000000, default 1. |
| `--no-euler-filter` | Preserve wrapped source Euler angles instead of unrolling equivalent +/-180-degree crossings. Explicit multi-turn values remain intact either way. |
| `--uv-channels` | Comma-separated source UV indexes 0–7 to retain; all eight are enabled by default. |
| `--uv-slot` | Repeatable `channel:semantic` mapping for FBX textures; e.g. `1:NormalMap`. |
| `--fbx-version` | ASCII/binary schema 7100, 7200, 7300, 7400 (default) or 7500. |
| `--cast-to-bone`, `--bone-size` | Represent transform-only FBX nodes as joints and set joint display size (default 10); geometry remains mesh geometry. |
| `--silent` | Suppress informational output; errors and partial-load diagnostics are still reported. |
| `--logger_flags` | Recognized legacy categories control informational output. |

Class, name, container, and PathID groups are combined with AND; alternatives within one
group are combined with OR. Put positional paths before filters accepting multiple
values. Invalid regexes and invalid filter files are errors, not skipped filters.
An explicit empty selection exports no objects. Model filters that select no
GameObjects or Animators fail instead of accidentally exporting the whole scene.

Use `--unity-cn-key` to supply an explicit UnityCN key, or select `--game UnityCN`
and `--key_index N` to use a key list. The default key file is `Keys.json` beside
the executable; `--keys-file` chooses another bounded JSON file. As in the
original application, UnityCN without an explicit key defaults to index zero.
Index and key validation never prints key material. The release package retains
the baseline public list under its original attribution.

Asset-map caches retain the original JSON list, XML layout, and MessagePack-CSharp
five-field record format, including its LZ4 block-array compression. The reader
also accepts legacy single-block LZ4 maps and the earlier Rust JSON/XML listings.
`--asset-map` detects the format automatically. Cached name, class, container, and
PathID filters run before opening sources; an empty selection loads no sources and
exports no objects. Matching a source file never implicitly selects all its assets.
Scene listing rejects asset filters; use `list` for a cached asset selection.

Sources must resolve inside the canonical asset base, including through symbolic
links. A relative source can resolve against that base or the current directory;
ambiguous paths fail. Relocate foreign absolute paths in the cache explicitly.
JSON/XML maps include optional serialized-source metadata so identical PathIDs in
two CABs inside one bundle remain distinct. Original binary maps lack that field:
ambiguous legacy identities fail to load, and binary writing rejects ambiguous
projections. Binary maps retain the game profile and require a matching `--game`.
CAB cache loading can additionally resolve dependencies for selected asset sources.

CAB `.bin` caches retain the original BinaryWriter layout. New caches describe
complete standard input files at offset zero; selecting any numeric split segment
still resolves the full segment group. Dependency lookup follows the complete
graph with case-insensitive CAB names, deduplicates cycles, and rejects missing
CABs. Source paths must remain below the canonical base folder, including through
symbolic links. Use `--cab-base` when moving a cache to another directory or OS.
Legacy caches with nonzero block offsets can be read, but loading those entries
fails explicitly: precise offsets inside proprietary decrypted block streams have
not been implemented. Source-level caches include all loaded serialized files;
asset filters narrow the asset list and export, rather than removing dependencies
from the cache.

JSON schemas are loaded before DLL-generated schemas. Embedded type trees remain
authoritative, and earlier external entries keep priority. Supply all matching
managed dependency assemblies when using `--dummy_dlls`. The application reads PE
metadata in an isolated Rust process; it does not execute managed code or require
a .NET runtime. Child output, memory inputs, schema complexity, and runtime are
bounded, and a failed reconstruction never produces a successful partial schema.

Existing files are preserved unless `--overwrite` is given for asset export,
container extraction, or CAB cache writing. Model and asset-map exports currently
preserve existing files. Loaded input files are protected even with `--overwrite`.
The `--image-format` switch selects PNG, JPEG, BMP, or TGA for converted textures
and sprites. Model output supports ASCII FBX, binary FBX, and OBJ with textures.

## Model and animation scope

Model controls also apply to converted GameObject/Animator asset exports.
Static-batched meshes retain each renderer's submesh subset, materials and
world/batch-root placement. Source normals, tangents, colors and UV0–7 survive
compaction. Synthetic format-22 files check nonuniform/negative transforms and
an unrelated batch root; these are not a real-game compatibility claim.

FBX UV semantics are Diffuse, NormalMap, Specular, Bump, Ambient, Emissive,
Reflection and Displacement (numeric aliases 0–7). Each source UV slot defaults
to Diffuse. A texture uses the first matching semantic emitted by every geometry
sharing its material, then falls back to a common existing UV set. If no UVs
exist, no named UV reference is written. Incompatible shared-material UV sets
fail explicitly. Different texture offsets/scales remain distinct even when
they share one image file. FBX 7500 uses 64-bit node headers; 7100–7400 use
32-bit headers. FBX 6.x is unavailable.

Converted AnimationClips produce Unity YAML `.anim` for explicit/packed legacy
curves and standard streamed/dense/constant modern bindings. Values remain in
Unity coordinates, with real paths/property names and reconstructed tangents.
Non-null object/script/event references fail until exported Unity GUID metadata
can remap them; null pointers remain null. Unresolved identities, separate
discrete streamed layouts and unmapped custom/external ACL streams also fail
explicitly. Raw/JSON export remains available for inspection.

The separate backend `acl::decode_compressed_tracks` API decodes supported
resident standard ACL samples; it does not automatically provide a game's
Unity binding map or enable opaque game ACL `.anim`/FBX export. See the
[feature table](../../docs/feature-parity.md) and
[game-profile compatibility matrix](../../docs/game-compatibility.md).

Numeric encrypted `.splitN` parts are joined lazily before wrapper decoding.
Selecting any part discovers the full contiguous group, and overlapping inputs
share discovery/read/decoded budgets. Preserve original logical filenames for
filename-derived keys. `--continue-on-error` cannot recover from shared resource
limit violations; it only skips individual format/I/O failures.

## Exit status

| Code | Meaning |
| --- | --- |
| `0` | Operation completed, including an empty filter result or skipped existing outputs. |
| `1` | Loading, parsing, filesystem, or other runtime failure. |
| `2` | Invalid arguments, incompatible switches, or a requested capability that has no implementation. |
| `3` | Some inputs or exports failed; successfully written outputs remain available. |

`--continue-on-error` allows a load to retain successfully read inputs. It does not
turn an incomplete load into exit status zero.

The process tests construct a small format-22 SerializedFile containing two
TextAssets. They verify the actual parser/exporter integration, selected export,
JSON listing, XML/JSON/MessagePack asset maps and exact cached identity loading,
empty cached selections, source confinement, map plus export, no-clobber and overwrite,
malformed options, partial export errors, empty model selections, CAB dependency
loading, both-map and all-output modes, original input protection, schema-file
loading, and the native schema helper protocol. Separate integration tests use an
actual generated managed DLL to recover a stripped MonoBehaviour through the Rust
child process. CAB module checks include independently encoded .NET wire bytes,
real UnityFS containers, transitive cycles, path confinement, and split groups.
Asset-map fixtures include independently generated MessagePack-CSharp output using
the baseline package version, signed ID limits, Unicode, and multiple LZ4 blocks.
Real game corpus coverage is separate from those synthetic fixtures.
