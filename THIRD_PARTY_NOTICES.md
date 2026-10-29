# Third-party notices

AssetStudio is distributed under the MIT license in `LICENSE`.

## Original implementation and format research

The original AssetStudio implementation and this fork credit Radu (2016),
Perfare (2016–2020), and Razmoth (2022). Their MIT notice remains in `LICENSE`.
The managed/C++ implementation at commit
`94318fa4eecb79190330da933bbc3d7de49055ed` is the migration reference.

## Rust Unity backend

The Rust application depends on `unity-rs-core` from
<https://github.com/seiunx-dev/unity-rs>, pinned to commit
`b3e509ed09fe6ff3a897d04ddb75eb5214482265`.
This MIT-licensed dependency provides Unity format parsing, decoding, and model
export. It is an existing independent Rust implementation, not code newly
authored in this rewrite.

Its license credits Radu (2016), Perfare (2016–2022), and aelurum (2021–2025).
Its format research credits AssetStudio by Perfare, AssetStudioMod by aelurum,
UnityPy, and vgmstream. Its vendored decoders have their own retained notices.
The release packager includes the exact pinned backend's LICENSE,
THIRD_PARTY_NOTICES.md, and THIRD_PARTY_LICENSES.txt alongside this notice.

## Rust dependencies

The full source dependency graph and exact revisions are recorded in
`Cargo.lock`. Eframe/egui supplies the native desktop interface; rfd supplies
native file dialogs; other Rust dependencies are declared in the crate
manifests. Native operating-system interfaces and third-party codec internals
are dependencies, not shipped .NET or Autodesk/FMOD helper DLLs.

No Autodesk FBX SDK, FMOD DLL, Oodle DLL, or .NET runtime is distributed by the
Rust package. Proprietary codec paths require separately verified replacements
before their old behavior can be claimed as equivalent.
