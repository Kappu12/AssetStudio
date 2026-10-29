# AssetStudio — Rust development branch

Native desktop browser and command-line tools for Unity assets. The Rust app
uses the pinned, MIT-licensed `unity-rs-core` format backend and runs without
.NET, Autodesk FBX SDK, or FMOD helper DLLs.

This branch is under active migration. It is not yet a feature-complete
replacement for every game-specific path in the original fork. See
[feature parity](docs/feature-parity.md),
[game compatibility](docs/game-compatibility.md), and
[the migration record](docs/rust-rewrite.md) for implementation and evidence.
The original implementation remains in this branch as a reference while its
remaining compatibility paths are ported; only the Cargo workspace is built and
packaged by the Rust workflow.

## Build and run

Install Rust with rustup. `rust-toolchain.toml` selects the tested toolchain.

```sh
cargo build --workspace --release --locked
cargo run -p assetstudio-gui --locked
cargo run -p assetstudio-cli --locked -- --help
```

On Linux, install the desktop/audio build dependencies first:

```sh
sudo apt-get install pkg-config libx11-dev libxkbcommon-dev libwayland-dev \
  libegl1-mesa-dev libgl1-mesa-dev libasound2-dev
```

The desktop binary is `AssetStudio` (`AssetStudio.exe` on Windows); the CLI is
`AssetStudioCLI`. Place both beside each other when using assembly-schema
helpers. GUI work includes multi-file/folder loading and drag/drop, asset
filtering and sorting, image/text/audio/mesh previews, scene hierarchy, selected
exports, and JSON/XML asset maps. File decoding and export run on workers.

## Command line

```sh
AssetStudioCLI game_Data exported --game Normal
AssetStudioCLI bundle --operation list --json
AssetStudioCLI bundle exported --types Texture2D Sprite --names '^Icon'
AssetStudioCLI bundle exported --export_type Raw --group_assets BySource
AssetStudioCLI bundle exported --write-map --map_type JSON
AssetStudioCLI bundle models --operation model --model-format fbx-binary
AssetStudioCLI bundle unpacked --operation extract
```

[CLI details and compatibility](crates/assetstudio-cli/README.md) describe legacy
aliases, filters, limits, and exit codes. Existing exports are preserved by
default. Parsing and conversion failures are reported explicitly; incomplete
loads and exports produce nonzero exit status.

## Verification

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
python -m unittest discover -s scripts/tests -v
python scripts/check_workflow_policy.py
```

Tests include actual constructed Unity serialized files and bundles, streamed
textures, malformed inputs, export selection, maps, schemas, model packages,
GUI state, and audio decoding. Synthetic tests and real game acceptance are
tracked separately. Test success is evidence for the tested cases, not a
zero-bug guarantee.

## Releases

CI builds and tests on GitHub-hosted Linux, Windows, and macOS runners. A matching
`v<workspace-version>` tag triggers the Windows x64 ZIP release only after all
validation jobs pass. Branch pushes and manual workflow runs do not release.
No release or release tag is being created during this migration.

The workflow uses **no Actions artifact uploads and no Actions cache storage**.
Each job builds on its own runner, and the release ZIP is uploaded directly to
GitHub Releases. See [release instructions](docs/releasing.md).

## Credits and licenses

Original AssetStudio research and implementations: [Perfare/AssetStudio](https://github.com/Perfare/AssetStudio),
Radu, and Razmoth. Rust backend: [seiunx-dev/unity-rs](https://github.com/seiunx-dev/unity-rs),
with credits to AssetStudioMod by aelurum and its other format contributors.
See [LICENSE](LICENSE) and [third-party notices](THIRD_PARTY_NOTICES.md).
