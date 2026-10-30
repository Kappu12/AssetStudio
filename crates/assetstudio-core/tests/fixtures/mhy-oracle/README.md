# Independent game-format compatibility oracles

These small synthetic golden outputs were produced by compiling exact sources
from baseline `94318fa`: `AES.cs`, `Mr0kUtils.cs`, `XORShift128.cs`,
`CryptoHelper.cs`, `LZ4.cs`, `LZ4Lit.cs`, and the unchanged
`DecryptLoveAndDeepspace` method extracted from `ImportHelper.cs` into
`LoveAndDeepspace.cs`. `Oracle.cs` supplies only a logger, byte-array extension,
minimal reader and configuration stubs. The repository MIT notice applies;
`AES.cs` retains its credit to Chris Hulbert's AES implementation.

Roslyn 4.11.0 (.NET SDK 8.0.419) compiled against the .NET 8 reference pack. The
oracle's runtime was .NET 8.0.25. It is temporary development infrastructure:
no shipped executable or normal `cargo test` invocation executes managed code.
Source/output SHA-256 values are recorded in `provenance.json`.

The seven basic mr0k inputs contain `unchecked((byte)(i*37+11))` at each byte,
with the first four bytes replaced by `mr0k`. Lengths cover a minimal header,
a nonempty XOR prefix, the full TOT post-key region, and the 1024-byte encrypted
block cap. Rust compares every output byte against the original C# outputs.
`Oracle.cs` also prints header-PRNG reference outputs for four seeds; those are
checked as exact numeric constants in Rust.

The `*-literal.mr0k` fixtures change only the initial compressed-stream literal
length header before original C# decryption. That decoder's resulting literal
payload is retained in `*-literal-payload.bin`, allowing real UnityFS block
reader tests to verify encrypted prefixes and declared block-size rewrites.

`LZ4Lit-reference.lit` and its original-C# decoded bytes exercise swapped token
nibbles, an extended match length, and overlapping back-references. LnD's
`.encrypted`/`.bin` pair checks the original 64-byte header-derived XOR stream.

To reproduce, compile the six original sources, `LoveAndDeepspace.cs` and
`Oracle.cs` with Roslyn's `csc.dll`, referencing the .NET 8 reference-pack DLLs.
Give the resulting program a `net8.0` runtimeconfig, then run
`dotnet Oracle.dll output-directory`. Direct Roslyn invocation does not need
the .NET SDK CLI frontend. No proprietary game data is included.
