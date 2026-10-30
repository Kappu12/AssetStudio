# Local Codex handoff

Continue on **rust-rewrite**. Do not push main, create a tag, or publish a
release. The owner explicitly deferred releases and wants natural checkpoint
commits. Subagents are authorized.

## State

The product is a Rust workspace: core, CLI, native egui desktop, and the patched
MIT unity-rs-core reader under vendor/. Original managed/native projects remain
as reference and oracle material; they are not built or shipped by the Rust
release workflow. This is still an incomplete compatibility rewrite, not a
zero-bug or full-parity claim.

Runtime checkpoint dfb4867 introduced modern standard .anim expansion, static
batch reconstruction, model/FBX settings, source-channel previews, embedded
class structures and five additional game variants. The following checkpoint
adds Arknights Endfield. See feature-parity.md and game-compatibility.md.

Latest registered workspace: **1,015 passed, 14 explicitly ignored optional
external oracle/corpus tests**. Strict workspace Clippy passed. All 37 Python
packaging/publishing/policy tests passed. Two independent source sweeps and
their fixes are recorded in review-sweeps.md and docs/reviews/.

dfb4867 passed a completely clean optimized Linux build and CLI/GUI smoke tests.
The Endfield optimized CLI also passed version/help and an actual profile probe.
Its latest optimized GUI link failed with a zero-byte generated ThinLTO object
after reusing one target directory across checkouts. A fresh complete target
build for the latest source is still required. Do not claim this is a confirmed
source fix, nor claim that the latest optimized GUI passed.

GitHub Actions runs were still zero when checked after dfb4867 was pushed. The
workflow is configured, but hosted Windows/MSVC/macOS validation is unverified.
Do not confuse local checks with hosted success. The current connector can
read runs and push Git objects but has no workflow-dispatch operation. Avoid
repeating the unsuccessful, long browser-authentication attempts.

## Unfinished legacy ACL work

The separate preservation commit retains these **unregistered** research files:

- vendor/unity-rs-core/src/acl_legacy.rs
- vendor/unity-rs-core/tests/fixtures/acl-legacy/
- crates/assetstudio-core/tests/fixtures/animation-mhy/

acl.rs does not include or export that decoder. Normal Cargo tests do not run
its test module. It was checked in a standalone Rust wrapper, strict lint and
mutation tests, against ten official ACL 1.1 compressor/C++ decoder fixtures
and the original x64 acl.dll. Three full/raw selectors are explicit negatives.
The synthetic MHY serialized fixtures have oracle values, but the application
parser and Unity animation adapters are not integrated or accepted yet.

Next steps: review/register the low-level decoder, run workspace tests/lints,
then implement the exact original C# MHY field/binding order and validate all
three complete .assets fixtures. Do not infer Unity mappings from standard ACL
headers. SR and database formats remain different and unsupported. The original
repository tracks acl/sracl/acldb DLLs under AssetStudio.GUI/Libraries; their
decoder source bodies are absent. The development PE oracle is documented beside
the legacy fixtures; no DLL/Wine/C/C++ dependency enters the Rust product.

## Remaining compatibility work

28/39 profiles are accepted. Eleven remain explicit errors:
GI, GI_Pack, GI_CB1, GI_CB2, GI_CB3, GI_CB3Pre, BH3, BH3Pre, ZZZ_CB1, SR_CB2, SR.
Their managed container algorithms/keys are present in the baseline. Implement
container and object-layout variants together before accepting a profile.
LoveAndDeepspace opaque animation payloads also remain unsupported. Other gaps
include non-null standalone-animation GUID remapping, dynamic SerializeReference
payloads, specialized CAB offsets, older/fork-specific layouts, and native
GUI/audio/platform acceptance. No real proprietary game corpus was supplied.

## Checks and delivery constraints

Use the pinned rust-toolchain.toml (1.99.1) and Python 3.11+:

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets --no-fail-fast --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
python -m unittest discover -s scripts/tests -v
python scripts/check_workflow_policy.py
cargo build --workspace --release --locked
```

Use a fresh target directory for the first optimized build. Smoke-test the built
CLI with --version/--help and desktop with --smoke-test (add .exe on Windows).
Linux desktop builds need X11/xkbcommon/Wayland/EGL/GL/ALSA development libraries;
the workflow lists the packages. Windows uses the MSVC toolchain/linker.

Release ZIPs upload directly to GitHub Releases only after a matching version
tag push and successful hosted checks. **No Actions artifacts or caches.**
Do not create a release/tag until the owner asks. Keep unsupported formats as
visible errors; do not substitute empty output or silently omit mesh geometry.
