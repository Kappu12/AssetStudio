# Independent AssetMap wire fixtures

`Oracle.cs` describes the original numbered MessagePack contracts: `AssetMap` is
`[GameType, AssetEntries]`, and an `AssetEntry` is
`[Name, Container, Source, PathID, Type]`. These fixtures were generated with the
baseline application's exact NuGet package, `MessagePack` **2.6.100-alpha**, from
MessagePack-CSharp commit `6d749218bb1790f7dde5689d8caa703f7d15986f` (MIT).
The oracle is test-only C#; neither Rust executable uses it or a managed runtime.

The small sample stays uncompressed under the C# serializer's size threshold.
The other samples cover LZ4 block-array extension 98, single-block extension 99,
Unicode, signed 64-bit limits, negative class IDs, and a 3,000-entry block array.
The checked-in JSON files are the C# serializer's independent expected records;
the large fixture's records follow the deterministic `Sample(3000)` generator.
The redundant 3,000-entry JSON is omitted; its retained binary is checked against
every field of the oracle's deterministic records by the Rust acceptance test.

To reproduce, obtain the pinned `MessagePack` and `MessagePack.Annotations`
packages and `Microsoft.NET.StringTools` 17.4.0 from NuGet. Compile `Oracle.cs`
against those DLLs and the .NET 8 reference pack using Roslyn's `csc.dll`; put the
three dependencies beside `Oracle.dll`. Use a .NET 8 runtime configuration and run:

```sh
dotnet Oracle.dll path/to/generated-fixtures
dotnet Oracle.dll --decode path/to/rust-generated.map
```

On 2026-10-29, the inverse check also passed: the Rust CLI generated a 3,000-entry
map with multiple LZ4 blocks, Unicode names, `i64::MIN` and `i64::MAX`. This exact
C# decoder read it, and every source, name, class ID and PathID matched the input.
Normal `cargo test` uses only the checked-in fixtures and requires no .NET SDK.
