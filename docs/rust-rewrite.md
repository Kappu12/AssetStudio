# Rust rewrite work log

Work is isolated to `rust-rewrite`. Do not tag or publish this branch yet.
The existing `main` remains the reference implementation.

## Baseline

Reference commit: `94318fa4eecb79190330da933bbc3d7de49055ed`.
The managed/native application contains 62,586 source lines across six .NET
projects and a C++ FBX helper. The rewrite replaces the application and helper
with Rust crates; there is no managed runtime in the new build.

## Planned checkpoints

1. Inventory, Rust workspace, dependency provenance, and shared interfaces.
2. Core loading, selection, safe export, scene operations, and fixtures.
3. Native desktop interface and compatible command-line interface.
4. Game-specific adapters and explicit compatibility accounting.
5. GitHub-hosted build/test jobs and tag-only release ZIP packaging, without
   Actions artifact uploads or cache storage.
6. Two review sweeps, regression fixes, validation results, and branch push.

## Compatibility rule

A feature is complete only after implementation and a corresponding check.
Synthetic-fixture checks and real-game checks must be recorded separately.
Missing fixtures or proprietary codecs must never be described as verified.
A stub or a raw-data fallback is not parity with an existing decoded export.

## Format backend

The backend is the MIT-licensed `unity-rs-core` 0.6.1, based on
`b3e509ed09fe6ff3a897d04ddb75eb5214482265` from
<https://github.com/seiunx-dev/unity-rs>. It provides native Rust Unity format
readers and exporters. Its source is now vendored in `vendor/unity-rs-core`,
patched by Cargo and tested as an explicit workspace member with one lockfile.
The upstream pin, local changes and source digest are separate provenance:
see the backend's `ASSETSTUDIO_PATCHES.md`. This repository owns the desktop app, CLI, selection,
grouping, compatibility adapters, packaging, and acceptance checks.
The original copyright notices and backend attribution must ship with packages.

## Validation record

The native Rust application and pipeline are implemented on the development
branch, with explicit remaining compatibility gaps. Checkpoint `6fad9ef` passed
213 application tests, strict Clippy, formatting, an optimized Linux build,
native-program smoke checks, Windows GNU cross-target checking and 32 pipeline
tests. These are historical results. The resumed frozen source has additional
backend, game, model, animation, class-structure and GUI regressions. At runtime
checkpoint `dfb4867`, the combined all-target workspace run passed 1,004 tests
with 14 explicitly ignored optional external oracles; strict workspace Clippy
and 37 Python delivery/policy tests passed. The optimized Linux release build
passed in a clean isolated worktree, along with native CLI/GUI smoke tests.
Two independent source sweeps and current addenda are recorded in
[review-sweeps.md](review-sweeps.md). No release has been created.

## Resumed compatibility work, 2026-10-30

- 28 of 39 game profiles now have accepted paths, including six added container
  and explicit serialized-field variants. Encrypted numeric split groups and
  multi-input wrapper budgets are shared across a whole operation.
- Static batching retains renderer submesh/root identities, compacts all vertex
  channels and reconstructs world/root-space placement. Synthetic parser fixtures
  and independent FBX imports verify nonuniform/negative owner transforms,
  material subsets and an unrelated batch root.
- Both FBX encodings support 7100–7500 with correct record widths, normal/color/
  tangent/UV0–7/material layers, matching imported object names, UV-role selection
  and per-binding texture transforms. GUI/CLI expose model filtering, scaling,
  skin/material/blend-shape controls, Euler unroll and bone display settings.
- Standard modern streamed/dense/constant animation bindings expand to `.anim`
  with bounded samples and resolved names. Non-null Unity reference GUIDs and
  unmapped custom/external ACL contracts remain explicit errors.
- A separate safe Rust backend API decodes standard resident ACL scalar/QVV
  samples; twelve genuine upstream ACL 2.1 compressed fixtures plus a durable
  96-case edge corpus with 123,300 sample comparisons establish that scope,
  without a C++ runtime dependency.
- The GUI has embedded class definitions, Preview/Dump/JSON switching and actual
  source normals/colors. Stale workers and settings are covered by targeted
  regressions. New independent reviews fixed wrapper-budget recovery, skin-node
  pruning, missing binary FBX layers, UV/texture binding corruption, dropped
  nonempty mesh failures and deep-hierarchy stack exhaustion.

The [feature table](feature-parity.md) and [game matrix](game-compatibility.md)
describe remaining ports and acceptance requirements. This work is not evidence
that every original-fork feature or current proprietary game build works.

## Published development checkpoints

- `ab1f02c`: feature inventory and dependency provenance.
- `206d3fd`: native Rust desktop/CLI/core checkpoint; 88 application tests and
  strict Clippy passed for this exact source snapshot.
- `281fd7a`: GitHub-hosted three-platform validation and tag-only ZIP packaging,
  without Actions artifact or cache storage. No tag or release was created.
- `157892d`: verified development checkpoint record.
- `4271053`: portable Windows CRT/PE checks, dependency license packaging,
  public key resource and game compatibility accounting.
- `6fad9ef`: native DLL schema recovery, CAB/AssetMap interoperability,
  font/static-model previews, animation exports, desktop controls and review
  regressions; 213 Rust and 32 pipeline tests pass.

Draft [PR #1](https://github.com/Kappu12/AssetStudio/pull/1) provides a review
surface without changing `main`. The prepared workflow has not yet produced a
GitHub-hosted run for this branch, including after PR creation; local success
must not be reported as a hosted runner result. The migration is not a complete
replacement for all 39 original game profiles. The feature/game matrices retain
the remaining ports and acceptance requirements.

The Endfield follow-up passed 1,015 workspace tests (14 optional external checks
ignored), strict workspace Clippy, 58 focused game units and four game Session
integrations. Its optimized CLI and actual Endfield profile probe passed; its
optimized GUI still needs a fresh target build after cross-checkout ThinLTO
cache corruption. See [local Codex handoff](LOCAL_CODEX_HANDOFF.md).

## Local Windows / Facepunch Rust continuation

The user prioritized the Facepunch Rust workflow; the other specialized-profile
ports remain deferred. See [Facepunch Rust usage and acceptance](facepunch-rust.md)
and the newest [feature evidence](feature-parity.md). Local checkpoints include
550e0e7 (Windows checks) and afa4382 (registered legacy ACL/BH3Pre, root motion
and catalog capacity). Later fixes cover actual Rust streamed clips, selected
model hierarchy scopes, real portable ZIP licensing and end-to-end Blender import.
The full 53.6 GB corpus loads 6,591,789 objects without diagnostics. The camper
model exports without failures and imports into Blender with all ten referenced
images present. Final Windows workspace validation passes 1,009 tests with 14
optional external tests ignored. No branch push, tag or release was requested.
