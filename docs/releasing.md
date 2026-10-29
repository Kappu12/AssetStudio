# Build and release workflow

The Rust workflow builds, lints, and tests the locked workspace on GitHub-hosted
Linux, Windows, and macOS runners. Pushes to `main` and `rust-rewrite`, pull
requests to either branch, and manual runs perform validation only.

Windows x64 builds statically link the MSVC runtime through the target-specific
Cargo configuration. The packager inspects both ordinary and delayed PE import
tables and refuses application runtime DLLs, including VC++ redistributables
and the former managed/native helpers. Windows 10/11 system DLLs and API sets
remain allowed, including the OS graphics and audio interfaces.

Development is currently on `rust-rewrite`. **Do not tag or release it yet.**
The compatibility and validation records must be reviewed before a release is
authorized; successful CI alone does not establish full parity with the former
managed application or compatibility with every game.

When a release is explicitly authorized:

1. Set `workspace.package.version` in `Cargo.toml` to the intended SemVer.
2. Commit the lockfile and changes, and let all three platforms pass CI.
3. Push a tag exactly matching `v<workspace.package.version>` on that commit.
4. The tag-triggered workflow validates the same source on all three platforms.
   Only after they succeed does its Windows release job rebuild the applications.
5. It packages `AssetStudio.exe`, `AssetStudioCLI.exe`, the baseline `Keys.json`
   registry, documentation, copyright notices, the backend's third-party license
   bundle, and a dependency inventory. It verifies the ZIP's contents and hashes
   and executes both packaged applications' smoke checks.
6. It creates or resumes a draft GitHub Release, uploads the ZIP and SHA-256 file,
   downloads those assets to verify their bytes, and then publishes. A version
   containing a SemVer prerelease suffix is marked as a prerelease. Failed uploads
   or checks leave a draft; published releases are never overwritten by a rerun.

The workflow uses **no Actions artifact uploads/downloads and no Actions caches**.
Build products, staging folders, and verification downloads exist only on each
runner's temporary disk and disappear with the runner. Release assets are uploaded
directly with the GitHub CLI; they are not stored in Actions artifact storage.
The only downloadable application package is the GitHub Release ZIP. CI logs
remain subject to the repository's ordinary log-retention settings.

Manual workflow runs cannot publish, and the publisher separately refuses all
events except an explicit pushed version tag. A mistyped or mismatched tag fails
before release creation. The workflow never creates or pushes tags itself.

The package verifier can be used locally on a Windows build:

```shell
cargo build --workspace --release --locked
python scripts/package_release.py package --smoke-test
```

Local package validation does not contact the release service. Package-policy
and release-guard tests run with:

```shell
python scripts/check_workflow_policy.py
python -m unittest discover -s scripts/tests -v
```

GitHub documents release assets separately from Actions artifacts in
[About releases](https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases).
The draft/upload/publish behavior and tag verification are documented in the
[GitHub CLI release manual](https://cli.github.com/manual/gh_release_create).
