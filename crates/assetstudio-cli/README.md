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
