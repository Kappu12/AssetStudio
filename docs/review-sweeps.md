# Rust rewrite review and validation record

The checkpoint results below are historical. The
[2026-10-30 addendum](#resumed-review-addendum-2026-10-30) records current work
and supersedes the old remaining-feature list without changing its evidence.

Runtime source checkpoint: `6fad9ef326844a667d468e67aa493294b4986ed8` on
`rust-rewrite`, reviewed on 2026-10-29. The original `main` reference remains
`94318fa4eecb79190330da933bbc3d7de49055ed`. No tag or release was created.

## Two source sweeps

Two different reviewers read all 38 first-party production Rust modules
(26 core, 5 CLI/helper, 7 GUI), plus tests, manifests, workflow and delivery
scripts. Changed files were reviewed again after fixes. Sweep 2's
[57-file SHA-256 ledger](reviews/sweep2-source-ledger.json) and
[detailed report](reviews/sweep2.md) retain its coverage and evidence.
The full application/test/build-script ledger contains 26,456 lines.

Relevant original C# layouts and backend integration paths were traced while
reviewing. This is not a new line-by-line audit of the external 120,000-line
Unity backend or every transitive dependency. Both reviewers finished with
no unresolved confirmed first-party defect in their inspected scope. That is
a review result, not a guarantee that every possible input is bug-free.

| Confirmed finding | Fix and regression evidence |
| --- | --- |
| Exports/extraction could overwrite original sources, companion inputs, hardlink aliases or auxiliary schema/index/cache/key inputs. Separately extracted inputs did not protect each other. | Shared source identities and auxiliary provenance, collective extraction and staged publication; exact collision tests preserve original bytes even with overwrite enabled. |
| Regex filter files were absent from overwrite protection. | Names/container filter provenance is retained; independent asset and CAB collision repros now preserve both filter and Unity input bytes. |
| Empty PathID-filtered model selection exported the entire scene. | Every explicit selection path participates in the model guard; actual CLI invocations return a no-roots error and create no output. |
| Stop during background audio decode could start playback later; malformed zero-rate/channel WAV headers could panic a decoder. | Audio generation tokens invalidate obsolete results and errors; bounded WAV validation precedes decoder/device creation, with device-free playback and mutation regressions. |
| Repeated hierarchy requests and stale filter/image/preview results crossed UI state changes. | Coalesced worker requests, generation/revision checks and iterative hierarchy preparation; targeted state/widget regressions pass. |
| An asset-index FIFO blocked before validation; collective extraction reset its output allowance per input. | Pre-open regular-file checks and a real named-pipe deadline regression; one shared cumulative extraction allowance. |
| Advisory game-wrapper warnings caused successful loads to exit as partial failures. | Warnings are separate from failed-input diagnostics; actual CLI success remains exit 0 and real partial loads remain exit 3. |
| Stripped MonoBehaviour Dump bypassed recovered schemas; Unity-6000 managed-reference Array alignment differed from the reference. | Schema-aware Dump and corrected alignment; actual stripped-object exports and all 60 independent layout comparisons pass. |
| Metadata expansion, identity mismatches and recursive types could defeat generator bounds or resolve the wrong assembly. | Owned bounded metadata projection, matching assembly identities and shared expansion/output budgets; malformed, recursive, timeout and native-child regressions pass. |
| Legacy empty-root paths lost FBX motion, and shared clips could lose their second owner. | Missing root tracks are repaired before writing, deduplicated by owner/clip; independent ufbx reads verify actual keys and duration in both FBX encodings. |
| Static-batched renderer subsets/root transforms were dropped by the backend model bindings. | Affected previews and OBJ/FBX exports now fail explicitly before publishing incorrect geometry. Complete reconstruction remains a recorded compatibility gap. |
| Material fallback JSON silently changed non-finite numbers to null. | Explicit finite-value validation and direct Material regression; schema/type-tree non-finite representations retain the backend's established behavior. |
| Font curve checks occurred after outline allocation, allowing repeated composite expansion. | Bounded SFNT/glyf preflight runs first; depth/work/raster limits and a tiny repeated-child fixture check early rejection. CFF/CFF2/gvar engines cannot bypass it. |
| Preferences could replace an original input or malformed/newer settings document. | Session provenance guard plus bounded atomic preferences; failures preserve prior bytes and never persist an explicit raw decryption key. |
| Platform path aliases/device names and pre-1980 dependency timestamps caused portability/packaging failures. | Canonical root handling, portable output names, ZIP timestamp normalization, static MSVC CRT policy and ordinary/delayed PE import validation, with regressions. |
| Packaged README links omitted their documentation targets. | Parity, review and CLI documentation are included at their linked paths; package-content tests verify bytes and relative navigation. |

## Final local checks

The parent ran the combined workspace checks after the fixes:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo build --workspace --release --locked
python -m unittest discover -s scripts/tests -v
python scripts/check_workflow_policy.py
```

| Check | Observed result |
| --- | --- |
| Rust tests | 213 passed: 126 core, 41 CLI, 46 GUI. The final test-only Container assertion was also rerun with all 7 AssetMap tests passing. |
| Format/check/strict Clippy | Passed for the combined workspace and all targets. |
| Linux optimized build | Both applications built successfully; CLI version/help/actual model catalog and GUI headless smoke checks passed. |
| Windows cross-target check | All workspace/test targets checked successfully for `x86_64-pc-windows-gnu`; this is not an MSVC build or Windows execution result. |
| Pipeline tests | 32 Python package/publisher/policy tests passed; workflow policy and actionlint passed. Publisher tests use a fake local GitHub CLI endpoint and create no remote release. |
| ZIP preflight | Windows dependency/license/resource inventory and ZIP manifests/checksums passed using PE fixture executables; actual native Windows executables still require a hosted/native build. |
| Hosted runners | No GitHub-hosted branch run has been observed yet, including after opening draft PR #1. Hosted results must not be inferred from local checks. |

## Independent format oracles

- Managed schemas: real synthetic DLL metadata and 20 independently generated
  reference layouts for each of Unity 5, 2019 and 6000. Three actual Rust child
  process tests recover stripped payloads without executing managed code.
- NetEase: the original C# implementation was compiled only as a temporary
  development oracle. Nine deterministic cases matched exactly; three golden
  outputs and provenance remain in the Rust test fixtures.
- AssetMap: genuine MessagePack-CSharp 2.6.100-alpha plain, single-block,
  block-array and multiblock fixtures. Its decoder also read Rust's 3,000-entry
  output and every field matched, including Unicode and signed 64-bit limits.
- FBX: ufbx 0.0.5 independently loaded generated ASCII and binary files. Each
  simple model has one triangle/material and translation keys at 0 and 1 second
  ending at `[-1, 2, 3]`. Explicit quaternion/Euler Z rotation reaches -90 degrees;
  packed rotation reaches approximately -89.94148 degrees at 0.5 seconds.
  A shared clip retains two owners and six XYZ translation curves. These are
  parsed key/time/value checks, not a claim of evaluated humanoid-pose coverage.

## Remaining work at the historical checkpoint

The [feature table](feature-parity.md) and
[game matrix](game-compatibility.md) distinguish implementation from evidence.
Seventeen specialized game profiles are still unimplemented. Some baseline ACL
paths call binary DLLs whose decoder source bodies are absent from the
repository; replacing those requires independent codec work and fixtures.
Static batches, modern standalone animation expansion/GUID remapping, specialized
CAB offsets, dynamic SerializeReference payload reconstruction, certain font
previews and advanced FBX controls remain incomplete.

No proprietary real-game corpus was supplied. This environment has no usable
native display/GPU/audio device for interactive acceptance. Headless widgets,
decoder/sink state tests and local cross-checks do not establish clean-machine
Windows/macOS interaction, physical playback, every codec or full original-fork
parity. The former implementation remains as reference and is not built or
included in the new application's release ZIP.

## Resumed review addendum, 2026-10-30

Scope: the resumed application changes, vendored-reader integration and targeted
backend changes, reviewed by separate feature owners and independent reviewers.
The old 57-file ledger and 213/32 test totals identify the earlier checkpoint;
they are not an inventory or final result for the expanded current workspace.
Runtime checkpoint: `dfb48672160c9d38a5cfbc6b405fbc268399bf7f`. The combined
`cargo test --workspace --all-targets --no-fail-fast --locked` run passed
**1,004 tests**, with **14 explicitly ignored optional external oracles**.
Strict workspace Clippy and all **37 Python packaging/publisher/policy tests**
passed. The clean optimized Linux build and native CLI/GUI smoke tests passed in an isolated worktree. These are local results; no remote release is exercised.

| Current work or finding | Implementation and review evidence |
| --- | --- |
| Game wrappers and variant layouts | 27/39 accepted profiles; Naraka/TOT/BH3PrePre/ExAstris/LoveAndDeepspace container/field variants, original C# byte oracles and real synthetic Session fixtures. Numeric split groups join lazily. A shared budget charges attempted encoded reads and retained outputs across selected paths; typed resource failures remain fatal through continue mode. |
| Static batching | Renderer ranges/root identities now survive parsing. Synthetic world/root-space scenes retain distinct subsets/materials and compact source channels. Independent ufbx imports verify actual positions under negative/nonuniform transforms. Nonempty unsupported mesh failures propagate instead of silently losing geometry. |
| FBX controls and layers | 7100–7500, bone/UV/model controls, both encodings' normal/UV/color/tangent/material layers and correct binary names. Independent ufbx 0.0.5 checks cover ten version/encoding files and six additional UV/texture-transform cases. Named UV references must exist on every geometry using the material; distinct per-binding offsets/scales remain distinct. |
| Skin pruning and deep graphs | Pruning resolves bone Transform identities and preserves hash-fallback requirements. Disabling skins converts the stripped renderer to static geometry; disabling materials removes references before parsing unsupported material bodies. Independent review reproduced a valid 10,000-node child-first graph aborting the old recursive FBX transform traversal; iterative traversal now reaches the ordinary bounded path error in both encodings. |
| Modern standalone animation | Standard streamed/dense/constant bindings expand to real `.anim` samples, paths, property names and tangents. Pointer tests preserve nulls and reject non-null references needing Unity GUID metadata. Custom/external ACL contracts remain errors. |
| Native standard ACL | Safe low-level resident sample API, twelve upstream ACL 2.1 compressed fixtures plus a durable 96-case edge corpus with 123,300 sample comparisons. Scalar/QVV mode, malformed-section, repaired-hash mutation and combined-allocation checks cover the supported scope; custom Unity maps are not inferred. |
| GUI and embedded classes | Source-normal/color painting, same-asset Preview/Dump/JSON revisions, full model controls/preferences, and asynchronous class-definition viewing/export. Class fixtures check versions/script identities, replacement precedence, text/resource ceilings and original-input preservation. Shutdown validates edited load/model settings before saving and preserves the previous loadable file when the draft is invalid. |
| Delivery provenance | The patched backend is a tested workspace member with one lockfile. ZIP inventory records origin plus a digest of the actual manifest/source tree, and includes its unchanged legal bundle and local modification record. No Actions artifacts or caches are used. |

See [sweep 2's current addendum](reviews/sweep2.md#resumed-targeted-review-2026-10-30)
for reviewer scope and reproducible FBX validation. Broad Rust 1.99 style cleanup
is distinct from functional review. No blanket lint suppression is used.

Current remaining work includes eleven rejected profiles, unmapped custom game
ACL/streaming/default-pose contracts, non-null standalone-animation GUID remapping,
specialized CAB offsets, dynamic SerializeReference payloads and the existing
font/media/platform acceptance gaps. No proprietary corpus was supplied. A
GitHub-hosted run has still not been observed; local tests are not MSVC/macOS
execution evidence. No tag or release has been created.

The reference does track `acl`, `sracl` and `acldb` DLLs under
`AssetStudio.GUI/Libraries`; their decoder source bodies are absent. Those
binaries can support future compatible Windows oracle work. They are not used
by the native Rust application. The vendored backend also retains its corpus
manifest and optional .NET oracle project at usable vendored paths; ignored
external-oracle tests are not counted as passing acceptance checks.

## Endfield follow-up

Arknights Endfield adds original-C# FairGuard/LZ4Inv oracles and explicit backend
layouts. Both-endian/truncation/budget cases passed. The combined all-target
workspace run passed 1,015 tests with 14 optional external checks ignored;
strict workspace Clippy passed. Focused evidence: 58 game units, four Session
checks and 727 backend units. The optimized CLI accepts the actual profile;
latest optimized GUI validation is pending a clean target rebuild. See
[handoff](LOCAL_CODEX_HANDOFF.md) for the exact cache issue and unfinished work.
