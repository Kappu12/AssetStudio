# Independent bug sweep 2

Reviewer: rust_gui. Scope: new first-party Rust application code, tests, manifests, GitHub workflow and release scripts. Sources were read in full, with subsequent changes reviewed again; tables/constants additionally received mechanical comparison against the C# baseline. This is not a full source audit of the pinned external Unity backend or every transitive dependency. A SHA-256/line-count inventory is in sweep2-source-ledger.json.

## Coverage ledger

| Area | Files read |
| --- | --- |
| Core application and export | lib.rs, catalog.rs, classes.rs, filter.rs, session.rs, export.rs, output.rs, source_protection.rs |
| Core formats and game wrappers | games.rs, game_bundle.rs, game_crypto.rs, game_constants.rs, game_objects.rs, recovery.rs, cab_map.rs, unity_cn_keys.rs |
| Core schemas and models | schema.rs, animation.rs, model.rs, model_motion.rs, model_preview.rs, font_preview.rs |
| Legacy asset maps | asset_map.rs, asset_map/json.rs, asset_map/messagepack.rs, asset_map/xml.rs |
| CLI | main.rs, schema_generator.rs, managed_metadata.rs, managed_serializer.rs, managed_templates.rs |
| Native GUI | main.rs, app.rs, view.rs, audio.rs, preview_image.rs, preview_geometry.rs, preferences.rs |
| Acceptance/process coverage | core acceptance.rs, application.rs, cab_map.rs, schema.rs, models.rs, asset_map.rs, font_preview.rs; CLI native_schema.rs and process.rs; inline unit tests in all above modules |
| Packaging and CI | all three crate manifests, Cargo.toml, .cargo/config.toml, .github/workflows/build.yml, scripts/package_release.py, scripts/publish_release.py, scripts/check_workflow_policy.py, scripts/tests/test_release_pipeline.py |

The seven Rust game constant arrays were compared byte for byte against CryptoHelper.cs/ImportHelper.cs and all match. The UnityCN resource equals the baseline byte for byte and retains all 23 entries. Class mappings were compared against the original enum; historic class-126 aliases are supported. Sentinel/primitive enum pseudo-values are not treated as real asset classes.

## Findings and disposition

| Severity | Finding and concrete evidence | Disposition |
| --- | --- | --- |
| P1 | Multiple separately extracted inputs could overwrite a different selected input or auxiliary file because each extraction guard saw only its own source. | Collective source identity guard and auxiliary provenance added; exact collision regressions pass. GUI now calls shared extract_paths_with_auxiliary_inputs and Session::write_cab_map. |
| P2 | Opening an asset-index FIFO blocked before file-kind validation. Old independent harness remained blocked; rebuilt fixed harness rejected in 0.0017 seconds. | Pre-open regular-file check and real named-pipe deadline regression pass. Later source-identity output checks also reject FIFOs before opening. |
| P2 | Successful FakeHeader list returned exit 3 solely due to an advisory profile warning, despite valid JSON assets. | Separate warnings/diagnostics APIs; actual CLI process regression passes success 0 with warning and real continued failure 3. GUI displays advisories without a failed-load banner. |
| P2 | Windows portable map names accepted reserved device variants with spaces/superscript COM/LPT suffixes. | CLI/CAB validation normalized to common output policy; boundary tests pass. |
| P2 | Stop during background WAV decoding did not invalidate the in-flight result, which could start playback afterward. | Audio request revision rejects stopped/obsolete results and errors, including returning to the same selected asset. Two direct GUI regressions pass without hardware. |
| P2 | Native schema differential fixture exposed ManagedRefs Unity-6000 Array alignment 0 instead of 0x4000. | Fixed generated metadata; all 60 independent schema snapshots pass, plus actual child-process array payload decoding. |
| P2 | Stripped MonoBehaviour dump bypassed loaded external/DLL schemas, unlike original GUI fallback. | Schema-aware value/tree dump added; schema acceptance tests include Dump and pass. |
| P2 | Shutdown preference writer could replace an original Unity input at the conventional settings path. | GUI consults Session source protection; helper refuses overwriting malformed/newer documents and preserves their bytes. |
| P2 | --operation model --path-ids 999999 exported scene/scene.fbx with exit 0 on the saved model fixture. | CLI omitted path-ID filters from whole-scene guard. Fixed and independently confirmed on the same real model fixture: exit 1, no files, correct no-roots diagnostic. All 22 process tests pass. |
| P2 | New material JSON fallback converted non-finite float/color/texture values to null via serde_json::json!. | Explicit finite-value validation added; direct Material regression passes for texture scale/offset, scalar floats and colors. |
| P1 | File-based name/container regex filters were not registered as source provenance and could collide with asset/CAB output. | CLI now registers canonical filter paths with Session protection; exact file-content preservation process tests pass. |
| P3 | Collective extraction reset its cumulative byte budget for each input. | One shared remaining allowance now threads through all inputs; tiny-resource regression proves the second cannot exceed the leftover budget. |
| P2 | Font outline limits ran after ab_glyph had allocated the expanded curve list; repeated composite children could expand exponentially within the dependency depth limit. | Bounded glyf/loca component/contour/point preflight now runs before outline decoding, shares per-glyph/sample budgets, and rejects alternative CFF/CFF2/gvar engines. Synthetic repeated-child and real-font tests pass. |
| Test defect | Static-model preview fixture used vec![0] for a named AnimationClip and failed during catalog construction before testing preview. | Replaced by valid name/truncated body; static GameObject/Animator world-geometry test now passes. |

## Final independent verification

- Native GUI: 46 unit/headless/transport/preferences tests passed after the preference source guard; strict all-target GUI Clippy passed. The native debug binary was built for parent Xvfb inspection. Parent is performing the final clean full-workspace run; no further Cargo jobs are queued by this reviewer. GameObject/Animator/font support arrives through existing Mesh/Image payloads, so no additional GUI stub or dispatch is required.
- Core: 126 tests passed in the final complete core run: 71 unit, 4 acceptance, 12 application/source-protection, 7 asset-map, 12 CAB, 2 actual embedded-font, 10 model/animation, and 8 external-schema tests. All confirmed core regressions above are included.
- CLI: final 22 process tests pass, including exact cached PathID/class selection, no-empty-selection imports, input/auxiliary/filter-file protection, advisories, and the model PathID guard. The real model fixture reproduction now returns exit 1 and writes no files. Final complete CLI run passes all 41 tests: 16 unit tests (including 60 metadata comparisons), 3 actual native-child tests, and 22 process tests.
- Packaging: all 32 package/publisher/workflow-policy Python tests passed. Those exercise direct release upload/download/hash verification with a fake local gh endpoint; they do not create a remote release.
- FIFO rejection was additionally checked by a separate subprocess harness, rather than relying only on implementation-adjacent tests.
- Parent-run independent ufbx 0.0.5 oracle loaded actual ASCII/binary root FBX (one mesh/material, three XYZ translation curves, two keys at 0 and 1), packed/quaternion/Euler root fixtures (six curves with actual rotation values/times), and the shared-owner fixture (two meshes, six translation curves). Endpoint translation was [-1, 2, 3]; quaternion/Euler Z rotation reached -90 degrees. Parent reports the Python evaluate_transform binding crashes, so no evaluated-matrix claim is made.

No unresolved confirmed source-safety, selection, stale-worker, or successful-data-loss finding remains in this reviewed snapshot.

The final source inventory contains 57 files and 26452 lines. Release packaging includes game compatibility notes, Keys.json and the vendored managed-schema MIT notice; direct package manifest hashes cover those files, and package/publisher policy tests pass.

## Material limits and evidence boundaries

No bug sweep can establish that every possible Unity asset or game build is bug-free. Real encrypted production corpora for the fork-specific profiles are not present, and wrapper compatibility advisories must remain. Specialized nonzero CAB block offsets and static-batched model preview/conversion are rejected explicitly. Static-batch subset/batch-root placement parity remains unimplemented. CFF/CFF2, variable TrueType, and bitmap-only font previews are unavailable; original font export remains available. Unsupported animation muscle/codec/layout paths report failures instead of writing fabricated output. The new static GameObject/Animator preview uses bounded untextured source/bind geometry, not an animated renderer. Native audio hardware playback, clean Windows runtime execution and macOS native interaction are not independently exercised by this reviewer; parent/hosted-runner results must be reported with their own provenance. External backend source is pinned and tested through fixtures but not claimed to have received this full first-party line-by-line audit.

No git commit, push, tag, release or PR was performed by this reviewer.

Final fixture-only review: the unused 635 KiB csharp_lz4_multiblock.json was removed; the actual C# multiblock binary and Oracle.cs remain. The existing acceptance loop now also compares every Container against Assets/UI/{index}.txt, completing all five legacy fields. Fixture README explains deterministic expected records and the omission. Runtime source hashes are unchanged. This reviewer did not queue another Cargo job; parent owns the narrow assertion rerun.

Final delivery-only review: package_release.py now includes docs/reviews/sweep2.md and docs/reviews/sweep2-source-ledger.json at their original paths, preserving the links in docs/review-sweeps.md. The existing package test creates both files and verifies their exact ZIP contents alongside the linked review index. Only these two Python source hashes changed; runtime source remains frozen. Parent reports all 32 final pipeline tests and policy checks pass. No further findings.
