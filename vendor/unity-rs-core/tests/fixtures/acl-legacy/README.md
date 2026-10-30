# Legacy MHY ACL oracle evidence

These synthetic clips were compressed and independently decoded by the untouched
MIT upstream ACL **v1.1.0**, commit
`bb2a8fa89ecd7d197b670809c5ac983b38051401`. Its public serialized format is
`compressed_clip`, tag `0xAC10AC10`, version **3**, with an integer sample rate
and five 16-bit relative header offsets. It differs from ACL 2.x
`compressed_tracks`. The Rust legacy decoder accepts the verified variable
drop-W quaternion/vector formats **4/3/3**. Full, fixed/raw format selectors,
SR, and database variants remain separate and unsupported.

Every positive `.acl` fixture was also decoded by the **original repository
x64 `acl.dll`**, SHA-256
`2982fbcf80062c2ad20ea7bcc5b15b601de68e4eb9f71e11689c3d7694cfe9c9`.
`manifest.json` records the compressed inputs and both oracle outputs. No game
assets or altered compressed headers were used. The matrix includes animated
single/multiple segments, default and constant tracks, mixed tracks, five and
twelve bones, bitsets crossing 32 bits, omitted scale storage, segment-local
constant samples, and raw 32-bit escape inside variable tracks.

Output is frame-major, then bone-major: rotation XYZW, translation XYZ, scale
XYZ. `.cpp.csv` rows begin with the frame index followed by ten values per bone.
`.dll.csv` has three rows: value/sample counts; sample times; flattened values.
Rust tests compare every value with both independent oracles and all times with
the original DLL. They also test repaired-hash malformed sections and counts,
truncations and deterministic mutations, non-finite ranges, unsupported formats,
and exact combined materialization budgets.

`legacy_fixture.cpp` and `variable_matrix.cpp` are development-only fixture
generators built against the pinned upstream headers and its included math
implementation. They are never built or linked by Cargo. The retained upstream
license is `LICENSE-ACL-1.1`.

The old upstream compressor leaves some unused header, alignment, bitstream and
end padding uninitialized. Re-running it can therefore change compressed-file
hashes while producing identical decoded samples. The manifest pins the captured
original bytes; neither the bytes nor their headers were normalized for testing.
DLL replay of those pinned inputs is reproducible.

`pe_oracle.c` is a development-only Linux x64 PE loader for the trusted pinned
DLL. It runs the actual DLL decoder with Microsoft-ABI CRT thunks and skips
Windows startup. Unsupported Windows/exception imports terminate the process.
It applies process CPU/address-space limits, input guard pages, allocation caps,
and validates output counts, live allocations and finite values. It is not a
general Windows runtime or a security boundary for arbitrary DLLs. The Rust
application has no DLL, Wine, C or C++ runtime dependency.

To reproduce the retained DLL outputs on Linux x64 with GCC:

```sh
python3 reproduce_dll.py
```

The script checks the exact DLL and fixture hashes before execution. `--write`
regenerates the DLL CSV files; ordinary execution compares them without writes.
