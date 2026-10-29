#!/usr/bin/env python3
"""Build and inspect release ZIPs on the runner's temporary disk.

This script never uploads anything. Only publish_release.py contacts GitHub,
and that script refuses to publish outside a version-tag push.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]
BINARIES = ("AssetStudio.exe", "AssetStudioCLI.exe")
DOCUMENTS = ("LICENSE", "README.md", "THIRD_PARTY_NOTICES.md")
BACKEND_DOCUMENTS = ("LICENSE", "THIRD_PARTY_NOTICES.md", "THIRD_PARTY_LICENSES.txt", "FSB_VORBIS_NOTICE.md")
SEMVER = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?\Z")


def workspace_version(root: Path = ROOT) -> str:
    version = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
    match = SEMVER.fullmatch(version)
    if not match or any(part.isdigit() and len(part) > 1 and part.startswith("0") for part in (match[4] or "").split(".")):
        raise ValueError(f"Invalid workspace SemVer: {version!r}")
    return version


def verify_tag(tag: str | None, root: Path = ROOT) -> str:
    version = workspace_version(root)
    if tag is not None and tag != f"v{version}":
        raise ValueError(f"Tag {tag!r} must equal workspace version v{version}")
    return version


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def cargo_metadata(root: Path = ROOT) -> dict:
    result = subprocess.run(
        ["cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", "x86_64-pc-windows-msvc"],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    return json.loads(result.stdout)


def backend_directory(metadata: dict) -> Path:
    matches = [package for package in metadata["packages"] if package["name"] == "unity-rs-core"]
    if len(matches) != 1:
        raise ValueError("Expected exactly one unity-rs-core dependency in Cargo metadata")
    return Path(matches[0]["manifest_path"]).parent


def require_file(path: Path) -> None:
    if path.is_symlink() or not path.is_file() or path.stat().st_size == 0:
        raise ValueError(f"Missing, empty, or symbolic-link package input: {path}")


def copy_dependency_licenses(metadata: dict, stage: Path) -> list[dict]:
    """Retain the legal files supplied in every external Cargo package."""
    fallback_root = ROOT / "packaging" / "licenses"
    fallback_manifest = fallback_root / "fallbacks.json"
    fallbacks = json.loads(fallback_manifest.read_text(encoding="utf-8")) if fallback_manifest.is_file() else {}
    inventory = []
    for package in metadata["packages"]:
        if not package.get("source"):
            continue
        package_root = Path(package["manifest_path"]).parent
        identity = package["name"] + "-" + package["version"]
        source_suffix = hashlib.sha256(package["source"].encode("utf-8")).hexdigest()[:8]
        destination = stage / "licenses" / "dependencies" / f"{identity}-{source_suffix}"
        legal_files = set()
        for entry in package_root.rglob("*"):
            if not entry.is_file():
                continue
            name = entry.name.lower()
            is_legal = any(word in name for word in ("license", "licence", "copying", "notice"))
            is_font_notice = "fonts" in entry.relative_to(package_root).parts and entry.suffix.lower() in (".txt", ".md")
            if is_legal or is_font_notice or name in ("ofl.txt", "ufl.txt"):
                legal_files.add(entry)
        if package.get("license_file"):
            explicit = package_root / package["license_file"]
            if explicit.is_file():
                legal_files.add(explicit)
        copied = []
        for path in sorted(legal_files):
            require_file(path)
            try:
                relative = path.resolve().relative_to(package_root.resolve())
            except ValueError as error:
                raise ValueError(f"Dependency legal file leaves its package: {path}") from error
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, target)
            copied.append(target.relative_to(stage).as_posix())
        fallback_sources = fallbacks.get(package["name"] + "@" + package["version"], [])
        for fallback in fallback_sources:
            source = fallback_root / fallback["path"]
            require_file(source)
            if sha256(source) != fallback["sha256"]:
                raise ValueError(f"Dependency fallback legal text changed without an audit: {source}")
            target = destination / "upstream" / source.name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, target)
            copied.append(target.relative_to(stage).as_posix())
        if not copied:
            raise ValueError(f"No redistribution legal files found for {identity}; add an audited upstream fallback")
        inventory.append({
            "name": package["name"], "version": package["version"],
            "license": package.get("license"), "source": package["source"],
            "license_files": copied,
            "fallback_sources": fallback_sources,
        })
    return sorted(inventory, key=lambda package: (package["name"], package["version"], package["source"]))


def check_executable(path: Path) -> None:
    require_file(path)
    with path.open("rb") as source:
        header = source.read(64)
        if len(header) < 64 or header[:2] != b"MZ":
            raise ValueError(f"Not a Windows executable: {path}")
        pe_offset = int.from_bytes(header[60:64], "little")
        if not 64 <= pe_offset <= path.stat().st_size - 6:
            raise ValueError(f"Invalid PE header offset: {path}")
        source.seek(pe_offset)
        if source.read(4) != b"PE\0\0":
            raise ValueError(f"Missing PE signature: {path}")
        if source.read(2) != b"\x64\x86":
            raise ValueError(f"Not a Windows x64 executable: {path}")


def inspect_zip(path: Path, expected_version: str) -> None:
    """Check archive contents and their hashes, without trusting archive paths."""
    with zipfile.ZipFile(path) as archive:
        names = archive.namelist()
        if len(names) != len(set(name.casefold() for name in names)):
            raise ValueError("ZIP contains duplicate or case-colliding names")
        for name in names:
            item = PurePosixPath(name)
            if item.is_absolute() or ".." in item.parts or "\\" in name or ":" in name:
                raise ValueError(f"Unsafe ZIP member: {name!r}")
            if not item.parts or archive.getinfo(name).is_dir():
                raise ValueError("ZIP must contain files only")
        required = set(BINARIES + DOCUMENTS + ("package-manifest.json",))
        required.update("licenses/unity-rs-core/" + name for name in BACKEND_DOCUMENTS)
        if not required.issubset(names):
            raise ValueError(f"ZIP is missing required files: {sorted(required - set(names))}")
        forbidden_suffixes = (".dll", ".pdb", ".cs", ".csproj", ".cpp", ".lib", ".sln")
        for name in names:
            if name.lower().endswith(forbidden_suffixes) or name.startswith(("target/", ".git/")):
                raise ValueError(f"Unexpected native/managed development input in package: {name}")
        manifest = json.loads(archive.read("package-manifest.json"))
        if manifest["version"] != expected_version or manifest["target"] != "win-x64":
            raise ValueError("ZIP version or target does not match the requested package")
        expected_names = set(names) - {"package-manifest.json"}
        if set(manifest["files"]) != expected_names:
            raise ValueError("ZIP manifest does not describe exactly its payload")
        for name, entry in manifest["files"].items():
            data = archive.read(name)
            if len(data) != entry["bytes"] or hashlib.sha256(data).hexdigest() != entry["sha256"]:
                raise ValueError(f"ZIP payload hash mismatch: {name}")


def make_package(
    build_dir: Path,
    output_dir: Path,
    *,
    root: Path = ROOT,
    tag: str | None = None,
    metadata: dict | None = None,
    smoke_test: bool = False,
) -> tuple[Path, Path]:
    version = verify_tag(tag, root)
    for name in BINARIES:
        check_executable(build_dir / name)
    for name in DOCUMENTS:
        require_file(root / name)
    metadata = metadata if metadata is not None else cargo_metadata(root)
    backend = backend_directory(metadata)
    for name in BACKEND_DOCUMENTS:
        require_file(backend / name)
    output_dir.mkdir(parents=True, exist_ok=True)
    zip_path = output_dir / f"AssetStudio-win-x64-v{version}.zip"
    checksum_path = output_dir / f"{zip_path.name}.sha256"
    with tempfile.TemporaryDirectory(prefix="assetstudio-stage-", dir=output_dir) as temporary:
        stage = Path(temporary)
        for name in BINARIES:
            shutil.copy2(build_dir / name, stage / name)
        for name in DOCUMENTS:
            shutil.copy2(root / name, stage / name)
        licenses = stage / "licenses" / "unity-rs-core"
        licenses.mkdir(parents=True)
        for name in BACKEND_DOCUMENTS:
            shutil.copy2(backend / name, licenses / name)
        for name in ("rust-rewrite.md", "compatibility.md", "releasing.md"):
            document = root / "docs" / name
            if document.is_file():
                require_file(document)
                (stage / "docs").mkdir(exist_ok=True)
                shutil.copy2(document, stage / "docs" / name)
        dependencies = copy_dependency_licenses(metadata, stage)
        (stage / "dependency-inventory.json").write_text(
            json.dumps(dependencies, indent=2) + "\n", encoding="utf-8", newline="\n",
        )
        files = {}
        for path in sorted(stage.rglob("*")):
            if path.is_file():
                files[path.relative_to(stage).as_posix()] = {"sha256": sha256(path), "bytes": path.stat().st_size}
        (stage / "package-manifest.json").write_text(
            json.dumps({"version": version, "target": "win-x64", "files": files}, indent=2) + "\n",
            encoding="utf-8", newline="\n",
        )
        if smoke_test:
            result = subprocess.run([str((stage / "AssetStudioCLI.exe").resolve()), "--version"],
                                    capture_output=True, text=True, encoding="utf-8", check=True, timeout=30)
            if version not in result.stdout:
                raise ValueError("Packaged CLI does not report the workspace version")
            subprocess.run([str((stage / "AssetStudio.exe").resolve()), "--smoke-test"],
                           capture_output=True, text=True, encoding="utf-8", check=True, timeout=30)
        temporary_zip = stage / "package.zip"
        with zipfile.ZipFile(temporary_zip, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
            for path in sorted(stage.rglob("*")):
                if path.is_file() and path != temporary_zip:
                    archive.write(path, path.relative_to(stage).as_posix())
        inspect_zip(temporary_zip, version)
        os.replace(temporary_zip, zip_path)
    checksum_path.write_text(f"{sha256(zip_path)}  {zip_path.name}\n", encoding="utf-8", newline="\n")
    return zip_path, checksum_path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="operation", required=True)
    version_parser = subparsers.add_parser("version")
    version_parser.add_argument("--tag")
    subparsers.add_parser("toolchain")
    package = subparsers.add_parser("package")
    package.add_argument("--tag")
    package.add_argument("--build-dir", type=Path, default=Path("target/release"))
    package.add_argument("--output-dir", type=Path, default=Path("dist"))
    package.add_argument("--smoke-test", action="store_true")
    package.add_argument("--github-output", type=Path)
    args = parser.parse_args()
    if args.operation == "toolchain":
        print(tomllib.loads((ROOT / "rust-toolchain.toml").read_text(encoding="utf-8"))["toolchain"]["channel"])
    elif args.operation == "version":
        print(verify_tag(args.tag))
    else:
        archive, checksum = make_package(args.build_dir, args.output_dir, tag=args.tag, smoke_test=args.smoke_test)
        print(json.dumps({"zip": str(archive), "checksum": str(checksum)}, indent=2))
        if args.github_output:
            with args.github_output.open("a", encoding="utf-8") as output:
                output.write(f"zip={archive.as_posix()}\nchecksum={checksum.as_posix()}\n")


if __name__ == "__main__":
    main()
