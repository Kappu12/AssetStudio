#!/usr/bin/env python3
"""Replay only pinned, synthetic legacy ACL fixtures through the original DLL."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import tempfile


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> None:
    folder = Path(__file__).resolve().parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dll", type=Path, default=folder.parents[4] / "AssetStudio.GUI/Libraries/x64/acl.dll")
    parser.add_argument("--write", action="store_true", help="regenerate retained DLL CSV files")
    args = parser.parse_args()
    manifest = json.loads((folder / "manifest.json").read_text())
    if sha256(args.dll) != manifest["dll_sha256"]:
        raise SystemExit("DLL differs from the pinned original MHY oracle")
    with tempfile.TemporaryDirectory(prefix="assetstudio-acl-oracle-") as temporary:
        executable = Path(temporary) / "pe-oracle"
        subprocess.run(["gcc", "-O2", "-Wall", "-Wextra", "-Werror", "-fno-strict-aliasing", str(folder / "pe_oracle.c"), "-lm", "-o", str(executable)], check=True, timeout=30)
        for fixture in manifest["fixtures"]:
            name = fixture["name"]
            if not name.replace("_", "").isalnum():
                raise SystemExit("invalid fixture name")
            source = folder / f"{name}.acl"
            cpp = folder / f"{name}.cpp.csv"
            target = folder / f"{name}.dll.csv"
            if sha256(source) != fixture["sha256"] or sha256(cpp) != fixture["cpp_csv_sha256"]:
                raise SystemExit(f"{name}: input or independent C++ oracle differs from manifest")
            data = source.read_bytes()
            bones = struct.unpack_from("<H", data, 16)[0]
            frames = struct.unpack_from("<I", data, 28)[0]
            if len(data) > 1024 * 1024 or not bones or not frames or bones * frames * 10 > 65536:
                raise SystemExit(f"{name}: fixture exceeds development replay limits")
            result = subprocess.run([str(executable), str(args.dll), str(source)], check=True, capture_output=True, text=True, timeout=5)
            if args.write:
                target.write_text(result.stdout)
            elif result.stdout != target.read_text():
                raise SystemExit(f"{name}: DLL output differs from retained evidence")
            if sha256(target) != fixture["dll_csv_sha256"]:
                raise SystemExit(f"{name}: retained DLL CSV differs from manifest")
            print(f"{name}: {bones} bones, {frames} frames verified")


if __name__ == "__main__":
    main()
