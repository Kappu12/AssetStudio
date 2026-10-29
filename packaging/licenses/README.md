# Redistribution legal inputs

Cargo packages sometimes omit their workspace's license texts from their
published crate. `fallbacks.json` records those missing legal texts with the
exact upstream crate VCS commit, source URL, and SHA-256 hash. The files here
are retained verbatim; `package_release.py` verifies their hashes before
copying them into a release ZIP. CC0's canonical text is from SPDX's immutable
license identifier instead of the crate's repository, which contains no
complete CC0 legal text.

Egui's 0.33.3 workspace members share one upstream VCS commit and its MIT and
Apache license choices. Their font package's own OFL, Ubuntu font, emoji and
Hack notices are also copied directly from its locked Cargo package. Existing
package-provided LICENSE/COPYING/NOTICE files are retained for all other
dependencies. Each ZIP's dependency inventory records exactly which legal
files and fallback sources it includes.

If a dependency update changes a recorded version, audit its legal inputs and
update the manifest. The packager refuses to ship a dependency with no legal
files instead of silently dropping its redistribution notices.
