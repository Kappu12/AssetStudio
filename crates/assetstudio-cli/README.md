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
AssetStudioCLI bundle models --operation model --model-format fbx-binary
AssetStudioCLI bundle unpacked --operation extract
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
| `--game` | Explicit format profile, defaulting to `Normal`; missing game-specific readers produce an error. |
| `--unity_version` | Override the engine version used to decode stripped-version files. |
| `--key`, `--xor_key` | MiHoYoBinData XOR byte, decimal or hexadecimal. |
| `--ai_file`, `--ai-file` | Read an asset-index JSON and recover container names before filtering or exporting. |
| `--group_assets`, `--group_assets_type` | `ByType`, `ByContainer`, `BySource`, or `None`. |
| `--export_type` | `Convert`, `Raw`, `Dump`, or `JSON`. |
| `--map_op AssetMap` | Write only the selected asset map. |
| `--map_type`, `--map_name` | JSON/XML and a portable filename; the output directory comes from the second positional argument. |
| `--silent` | Suppress informational output; errors and partial-load diagnostics are still reported. |
| `--logger_flags` | Recognized legacy categories control informational output. |

Class, name, and container groups are combined with AND; alternatives within one
group are combined with OR. Put positional paths before filters accepting multiple
values. Invalid regexes and invalid filter files are errors, not skipped filters.
An explicit empty selection exports no objects. Model filters that select no
GameObjects fail instead of accidentally exporting the whole scene.

Use `--unity-cn-key` to supply an explicit UnityCN key. The old bundled-key-index
selector `--key_index`, managed assembly loading through `--dummy_dlls`, and CABMap
cache operations (`--map_op CABMap`, `Both`, and cache-loading variants) are not
implemented. They fail with a usage error instead of being silently ignored.

Existing files are preserved unless `--overwrite` is given for asset export or
container extraction. Model and map exports currently preserve existing files.
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
JSON listing, XML/JSON asset maps, map plus export, no-clobber and overwrite,
malformed options, partial export errors, and empty model selections. Real game
corpus coverage is separate from those synthetic fixtures.
