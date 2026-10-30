# Compatibility resources

`Keys.json` is copied unchanged from `AssetStudio/Keys.json` at the baseline
commit `94318fa4eecb79190330da933bbc3d7de49055ed` of this repository. It is the
existing public UnityCN compatibility list, retaining the original index order
and names. The upstream AssetStudio/Razmoth attribution and MIT license in the
repository apply; no private account credentials were added.

Release packages place this file beside both native executables as `Keys.json`.
The Rust reader bounds the file, validates every key, and redacts key values in
diagnostics. Users may select their own file explicitly with `--keys-file`.
