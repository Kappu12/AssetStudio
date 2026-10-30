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

All accepted wrapper profiles expose a visible advisory through
`Session::warnings()` explaining that real-game compatibility is not verified.
These advisories do not count as load failures or change a successful CLI exit
status. Input files are never modified.

The current selector accepts **27 of the 39** baseline profiles. Twelve remain
explicit errors: GI/GI_Pack/GI_CB1/GI_CB2/GI_CB3/GI_CB3Pre, BH3/BH3Pre,
ZZZ_CB1, SR_CB2/SR and ArknightsEndfield. Acceptance of a profile covers the
implemented container and field variants below, not every historical layout or
its opaque animation codec.

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
| BH3PrePre | Historical encrypted header and pack-key mr0k blocks; player-layout override implemented | Original C# byte oracles and synthetic Session fixtures; other BH3 versions remain separate |
| ZZZ_CB1 | Not implemented | mr0k compression and custom serialized/animation layouts |
| SR_CB2 | Not implemented | mr0k compression, alignment and custom serialized/animation layouts |
| SR | Not implemented | mr0k compression, alignment and custom serialized/renderer/animation layouts |
| TOT | mr0k/post-key blocks, concatenated bundles and player-layout override implemented | Original C# byte oracles and synthetic Session fixtures; custom unmapped animation codecs remain unsupported |
| Naraka | Signature/header normalization, block-size reconstruction and material field variant implemented | Synthetic Session TextAsset/Material checks; live game not tested |
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
| LoveAndDeepspace | Header XOR plus explicit mesh/material/controller/shader and animation-envelope layouts implemented | Original header-XOR oracle and synthetic parser/Session fixtures; opaque custom animation decoding remains unsupported |
| SchoolGirlStrikers | Alternating-stride byte XOR implemented | Exact synthetic output comparison |
| ExAstris | LZ4Lit blocks, texture/mesh fields, animation header and resident dense quantization implemented | Original LZ4Lit oracle and synthetic parser/Session fixtures; 8/16/24/32-bit dense samples are bounded, nonzero generic quantization groups and unmapped custom ACL remain unsupported |
| PerpetualNovelty | Encrypted block-table LZ4 prefix implemented | Synthetic front/end-table UnityFS checks; live game not tested |

## Shared safeguards

Game wrappers are transformed entirely in Rust. All selected paths share
cumulative 512 MiB allowances for attempted encoded materializations and retained
decoded wrappers. Failed decode attempts consume read work too. Conversion can
temporarily hold multiple buffers,
so the ceiling is not a claim about peak memory. Standard inputs and companion
resources remain file-backed. Directory discovery limits files, directories,
depth and path length, and rejects symbolic links. A malformed or unknown
encrypted header returns an error. Resource names remain unchanged, so normal
`.resS` and `.resource` lookup keeps working.

With the explicit continue-on-error load option, an individual game file's I/O
or format failure is recorded as a visible diagnostic while other files can
load. Shared directory, diagnostic and cumulative materialization limits still
abort the operation; the option cannot bypass those limits.

Normal/UnityCN split-input handling is provided by the standard backend. Game
wrapper `.splitN` groups are discovered, sorted numerically and concatenated
lazily before decoding. Selecting a later segment still resolves the complete
group. Missing/duplicate indexes and simultaneous complete/split inputs fail
explicitly; operation-wide source/read/decoded budgets also cover overlapping
folder and multi-file selections. Filename-derived keys use the original
logical filename, so renaming encrypted inputs can prevent decryption.

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

The backend now has a separate native API for resident standard ACL samples:
`unity_rs_core::acl::decode_compressed_tracks`. It accepts release scalar
formats 7/10 and resident ACL 2.1 QVV format 10. Independent upstream ACL 2.1
fixtures check actual samples. That does not map Ex Astris, Tuanjie or other
custom Unity-fork curve indexes/bind poses into model or `.anim` output.
External database/stream data, stripped keyframes, external default poses and
development format versions 8/9 remain unsupported. Application exports still
report unsupported custom ACL contracts explicitly.

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

Additional exact C# oracles for expanded-key AES/mr0k, LZ4Lit and the Love and
Deepspace header transform are retained with hashes in
`crates/assetstudio-core/tests/fixtures/mhy-oracle/`. Naraka, TOT, BH3PrePre,
ExAstris and LoveAndDeepspace integration fixtures run through actual Session
loading and object conversion. All of these inputs are synthetic.

Love and Deepspace mesh conversion covers uniform packed morph records;
mixed classic/packed or variable-width records and reserved layouts fail
explicitly. Its shader parser validates and retains secondary code metadata,
while text conversion uses the standard primary program blob. Parsing opaque
secondary storage is not decompilation of that code. Empty custom animation
envelopes can retain verified standard curves; nonempty custom caches/maps/ACL
payloads still require a separately verified decoder.

## Provenance

The wrapper algorithms and game constants were transcribed from
`AssetStudio/ImportHelper.cs`, `AssetStudio/Crypto/CryptoHelper.cs`, and
`AssetStudio/GameManager.cs` at the baseline commit. The repository MIT license
and upstream author notices apply to those translated portions. Generic Unity
parsing remains attributed to the separately pinned MIT `unity-rs-core` backend.
