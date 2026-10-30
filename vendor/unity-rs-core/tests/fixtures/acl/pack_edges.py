#!/usr/bin/env python3
"""Pack exact upstream C++ edge-oracle outputs without timestamps or source paths."""

import argparse
import csv
import hashlib
import io
import json
import math
from pathlib import Path
import zipfile

TRACK_COUNTS = (1, 2, 3, 4, 5, 15, 16, 17)
SOURCE_FRAMES = (1, 11, 37)
FORMATS = {
    0: "variable rotation and vectors",
    1: "full precision rotation and vectors",
    2: "full precision dropped-W rotation; variable vectors",
    3: "variable rotation and vectors; every scale is identity",
}


def member(name: str, data: bytes) -> tuple[zipfile.ZipInfo, bytes]:
    info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
    info.compress_type = zipfile.ZIP_DEFLATED
    info.create_system = 3
    info.external_attr = 0o100644 << 16
    return info, data


def pack(source: Path, output: Path) -> None:
    cases = {}
    files = {}
    for tracks in TRACK_COUNTS:
        for source_frames in SOURCE_FRAMES:
            for encoding in FORMATS:
                name = f"edge_{tracks}_{source_frames}_{encoding}"
                encoded = (source / f"{name}.acl").read_bytes()
                samples = (source / f"{name}.csv").read_bytes()
                if len(encoded) > 64 * 1024 or len(samples) > 128 * 1024:
                    raise ValueError(f"{name}: source exceeds the test's member limit")
                rows = list(csv.reader(io.StringIO(samples.decode("ascii"))))
                track_count, stored_frames, width = map(int, rows[0][:3])
                if not (
                    track_count == tracks
                    and width == 10
                    and 1 <= stored_frames <= source_frames
                    and float(rows[0][3]) == 30.0
                    and len(rows) == stored_frames + 1
                ):
                    raise ValueError(f"{name}: unexpected upstream output dimensions")
                for row in rows[1:]:
                    if len(row) != tracks * 10 or not all(math.isfinite(float(v)) for v in row):
                        raise ValueError(f"{name}: incomplete or non-finite upstream sample")
                if int.from_bytes(encoded[:4], "little") != len(encoded):
                    raise ValueError(f"{name}: upstream ACL size does not match its bytes")
                cases[name] = {
                    "tracks": tracks,
                    "source_frames": source_frames,
                    "stored_frames": stored_frames,
                    "encoding": encoding,
                    "sha256": {
                        "acl": hashlib.sha256(encoded).hexdigest(),
                        "csv": hashlib.sha256(samples).hexdigest(),
                    },
                }
                files[f"{name}.acl"] = encoded
                files[f"{name}.csv"] = samples
    present = {p.name for p in source.iterdir() if p.suffix in (".acl", ".csv")}
    if present != files.keys():
        raise ValueError("input must contain exactly the 96 expected ACL/CSV pairs")
    manifest = {
        "schema_version": 1,
        "acl_commit": "414689d5cff4286a7898487a46dc5e48005d38da",
        "rtm_commit": "d7982f2b2524feeb322f424b29cf43df30b5d5b7",
        "generator_sha256": hashlib.sha256(Path(__file__).with_name("edge_oracle.cpp").read_bytes()).hexdigest(),
        "components": "quaternion XYZW, translation XYZ, scale XYZ",
        "sample_rate": 30,
        "formats": FORMATS,
        "cases": cases,
    }
    files["manifest.json"] = (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode("utf-8")
    if sum(map(len, files.values())) > 2 * 1024 * 1024:
        raise ValueError("fixtures exceed the test's shared uncompressed byte limit")
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for name, data in sorted(files.items()):
            info, contents = member(name, data)
            archive.writestr(info, contents, compresslevel=9)
    print(f"Packed {len(cases)} edge cases: {output.stat().st_size} bytes")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, help="directory produced by edge_oracle.cpp")
    parser.add_argument("output", type=Path, help="destination ZIP")
    arguments = parser.parse_args()
    pack(arguments.source, arguments.output)
