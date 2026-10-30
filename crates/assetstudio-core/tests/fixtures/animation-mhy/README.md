# Serialized MHY animation fixtures

These synthetic Unity 2022.3.62 serialized format-22 player files follow the
original C# `AnimationClip`/`Clip`/`MHYACLClip` field order. The `NoTarget` platform
metadata deliberately exercises the explicit MHY player-layout override.

The embedded `.acl` payloads are unchanged outputs of the upstream ACL v1.1.0
compressor. Each exact embedded payload was independently decoded with both its
upstream C++ decoder (`.cpp.csv`, first column frame index) and the original
repository x64 `acl.dll` (`.dll.csv`, counts/times/flattened-values rows).
`manifest.json` records source hashes, binding order, expected following dense
and constant samples, and the exact original DLL hash.

`mhy_variable` has one bone and real animated R4/T3/S3 tracks. `mhy_mixed_tracks`
adds a real child hierarchy with following dense position and constant scale
tracks to verify binding offsets. `mhy_multibone` has a three-bone hierarchy with
animated, constant, and default tracks, using actual hierarchy CRC paths.

`generate.py` regenerates the `.assets` and `.object` bytes from the retained
`.acl` payloads. It uses only the Python standard library. The legacy codec's
upstream license and independent DLL replay harness are retained in the backend
`tests/fixtures/acl-legacy` directory. No DLL is linked or invoked by Cargo.
