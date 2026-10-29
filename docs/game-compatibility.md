# Game profile compatibility

This matrix compares the Rust branch with the `94318fa` managed/native fork.
The presence of a profile in the old menu was not evidence that every release
of that game worked. The same rule applies to the Rust implementation.

The default `Normal` profile is the right choice for ordinary Unity games,
including ordinary Facepunch Rust bundles. It does not need a game-specific
key. Unity engine version and platform codec support still depend on the
pinned format backend and the actual input.

## Evidence and scope

- **Implemented** means a Rust code path exists. For wrapper profiles it
  transforms the outer bytes and then uses the standard Unity parser.
- **Synthetic checks** exercise a small constructed input or malformed byte
  sequences. They do not establish compatibility with a live game build.
- **Not implemented** profiles are rejected explicitly before loading. They
  are never silently routed through `Normal` or described as decoded exports.
- No real game corpus was supplied for this rewrite. Every named proprietary
  game's current real-world compatibility is **Not tested**.

All accepted wrapper profiles produce a visible load diagnostic explaining
that real-game compatibility is not verified. Input files are never modified.

| Legacy profile | Rust path | Remaining evidence or implementation |
| --- | --- | --- |
| Normal | Standard Unity backend | Per-class engine/platform limits remain; verify actual assets |
| UnityCN | Backend caller-supplied 16-byte key | Correct key and representative encrypted bundle required |
| GI | Not implemented | BLK/mhy0 containers, custom type tables, renderer/shader/animation/mesh layouts, asset-index recovery |
| GI_Pack | Not implemented | Pack/mr0k chunks and custom serialized/object layouts |
| GI_CB1 | `mark` envelope primitive only; profile rejected | Custom serialized, renderer, mesh and animation layouts |
| GI_CB2 | Not implemented | BLK and custom serialized/object layouts |
| GI_CB3 | Not implemented | BLK and custom serialized/object layouts |
| GI_CB3Pre | Not implemented | BLK/mhy0 and custom serialized/object layouts |
| BH3 | Not implemented | Encrypted bundle headers, mr0k blocks, custom renderers/meshes/animations |
| BH3Pre | Not implemented | Encrypted bundle headers, mr0k blocks, custom renderers/animations |
| BH3PrePre | Not implemented | Different encrypted bundle header order |
| ZZZ_CB1 | Not implemented | mr0k compression and custom serialized/animation layouts |
| SR_CB2 | Not implemented | mr0k compression, alignment and custom serialized/animation layouts |
| SR | Not implemented | mr0k compression, alignment and custom serialized/renderer/animation layouts |
| TOT | Not implemented | mr0k post-key and custom serialized layout |
| Naraka | Not implemented | Replaced bundle signature, block size adjustment, material layout |
| EnsembleStars | Outer byte transform implemented | Synthetic transform checks; live game not tested |
| OPFP | Fake-header plus per-compressed-block V0/V1 path-keyed transforms implemented | Synthetic front/end-table UnityFS checks; original `BundleResources` path required for encrypted paths |
| FakeHeader | First/second UnityFS-header recovery implemented | Synthetic prefix and malformed-input checks |
| FantasyOfWind | Footer-keyed encrypted prefix implemented | Synthetic transform checks; live game not tested |
| ShiningNikki | Same fake-header recovery | Synthetic prefix checks; live game not tested |
| HelixWaltz2 | Seeded substitution wrapper implemented | Historical .NET PRNG reference sequence and synthetic wrapper checks |
| NetEase | CRC-keyed compressed-block decoding and nonstandard RC4 implemented | Nine exact comparisons against the original C# algorithm; three retained golden outputs; live game not tested |
| AnchorPanic | Filename-MD5 keyed RC4 chunks implemented | RC4/MD5 reference vectors and synthetic wrapper checks |
| DreamscapeAlbireo | Header reconstruction and block-table XOR implemented | Declared sizes validated; live game not tested |
| ImaginaryFest | Single-byte XOR, path-offset and path-keyed variants implemented | Path under original `data` directory required for path-keyed variants |
| AliceGearAegis | Header reconstruction and prefix XOR implemented | Synthetic transform checks; live game not tested |
| ProjectSekai | Version 0x10/0x20 wrappers implemented | Exact synthetic output comparison and truncated prefix checks |
| CodenameJump | Repeating XOR wrapper implemented | Exact synthetic output comparison |
| GirlsFrontline | Header-derived XOR prefix implemented | Synthetic transform checks; live game not tested |
| Reverse1999 | Filename-derived byte XOR implemented | Exact synthetic output comparison; original filename required |
| ArknightsEndfield | Not implemented | LZ4Inv blocks, BLB containers, first-block encryption and animation layouts |
| JJKPhantomParade | Generation-1 AES-256 counter wrapper implemented | Synthetic header/counter checks; current game versions not tested |
| MuvLuvDimensions | Repeating XOR wrapper implemented | Exact synthetic output comparison |
| PartyAnimals | Filename-derived tail XOR implemented | Original filename required; synthetic transform checks |
| LoveAndDeepspace | Not implemented | Header XOR plus mesh/material/controller/shader/animation variants |
| SchoolGirlStrikers | Alternating-stride byte XOR implemented | Exact synthetic output comparison |
| ExAstris | Not implemented | LZ4Lit blocks and texture/mesh/ACL animation variants |
| PerpetualNovelty | Encrypted block-table LZ4 prefix implemented | Synthetic front/end-table UnityFS checks; live game not tested |

## Shared safeguards

Game wrappers are transformed entirely in Rust. Retained decoded wrappers have
a cumulative 512 MiB ceiling per operation; each encoded input is also bounded
before materialization. Encoding conversion can temporarily hold both copies,
so the ceiling is not a claim about peak memory. Standard inputs and companion
resources remain file-backed. Directory discovery limits files, directories,
depth and path length, and rejects symbolic links. A malformed or unknown
encrypted header returns an error. Resource names remain unchanged, so normal
`.resS` and `.resource` lookup keeps working.

With the explicit continue-on-error load option, an individual game file's I/O
or format failure is recorded as a visible diagnostic while other files can
load. Shared directory, diagnostic and cumulative materialization limits still
abort the operation; the option cannot bypass those limits.

Normal/UnityCN split-input handling is provided by the standard backend.
Combining encrypted game-specific `.splitN` parts before wrapper decoding is
not implemented on this branch. Join those original parts first; keep source
files unchanged and retain the original filename when the key depends on it.

MiHoYoBinData class 1208 uses an optional XOR byte. That simple object payload
operation does not imply support for the GI/BH3/SR custom containers. IndexObject
class 1210 has a bounded native parser for name/reference/size records, including
32/64-bit PathIDs, both byte orders and the editor-only prefab-pointer prefix.
Its fixtures check all those combinations and absolute string alignment.
Offline asset-index name recovery and compatible CAB cache parsing are
implemented separately. Legacy CAB offsets into decrypted/decompressed streams
are rejected explicitly; they cannot be treated as ordinary source offsets.
Asset-index remote download and all game-specific object-layout equivalence
still require separate implementation and evidence.

The OPFP, NetEase and PerpetualNovelty transforms operate on UnityFS versions
6–8. They retain original block sizes and directory bytes and decrypt only the
fork's documented encrypted portions. Metadata decompression supports None,
LZMA and LZ4/LZ4HC; metadata bytes and LZMA decoder memory each have a 64 MiB
ceiling. The normal backend
then validates the full directory, block payloads and extracted object layouts.

For NetEase, an isolated temporary .NET development oracle compiled the exact
baseline `NetEaseUtils.cs` and produced byte-for-byte matching results for raw
regions of 32, 80, 160, 255, 256, 800, 4096 and 4100 bytes, and one 800-byte
header-bearing region. Synthetic inputs, selected golden outputs, the baseline
source hash and provenance are recorded in
`crates/assetstudio-core/tests/fixtures/game-oracle/`. The shipped app and CI
tests remain Rust and do not execute .NET.

## Provenance

The wrapper algorithms and game constants were transcribed from
`AssetStudio/ImportHelper.cs`, `AssetStudio/Crypto/CryptoHelper.cs`, and
`AssetStudio/GameManager.cs` at the baseline commit. The repository MIT license
and upstream author notices apply to those translated portions. Generic Unity
parsing remains attributed to the separately pinned MIT `unity-rs-core` backend.
