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

The candidate backend is the MIT-licensed `unity-rs-core`, pinned to
`b3e509ed09fe6ff3a897d04ddb75eb5214482265` from
<https://github.com/seiunx-dev/unity-rs>. It provides native Rust Unity format
readers and exporters; this repository owns the desktop app, CLI, selection,
grouping, compatibility adapters, packaging, and acceptance checks.
The original copyright notices and backend attribution must ship with packages.

## Validation record

Implementation and verification are in progress. No release has been created.

## Published development checkpoints

- `ab1f02c`: feature inventory and dependency provenance.
- `206d3fd`: native Rust desktop/CLI/core checkpoint; 88 application tests and
  strict Clippy passed for this exact source snapshot.
- `281fd7a`: GitHub-hosted three-platform validation and tag-only ZIP packaging,
  without Actions artifact or cache storage. No tag or release was created.

Additional compatibility ports and both review sweeps are in progress. The
prepared CI workflow has not yet produced a GitHub-hosted run for this branch;
local test success must not be reported as a hosted runner result.
