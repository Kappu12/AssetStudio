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
import struct
import subprocess
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]
BINARIES = ("AssetStudio.exe", "AssetStudioCLI.exe")
DOCUMENTS = ("LICENSE", "README.md", "THIRD_PARTY_NOTICES.md")
PACKAGE_RESOURCES = {"Keys.json": "resources/Keys.json"}
VENDORED_LICENSES = {
    "licenses/managed-schema-templates/LICENSE": "packaging/licenses/managed-schema-templates/LICENSE",
}
BACKEND_DOCUMENTS = ("LICENSE", "THIRD_PARTY_NOTICES.md", "THIRD_PARTY_LICENSES.txt", "FSB_VORBIS_NOTICE.md")
BACKEND_LOCAL_DOCUMENTS = ("ASSETSTUDIO_PATCHES.md",)
BACKEND_PATH = "vendor/unity-rs-core"
SEMVER = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?\Z")
# Imported Windows components are supplied by Windows 10/11. Graphics and audio
# drivers may load their own modules dynamically through these OS interfaces.
# Application runtimes (VC++, .NET, FMOD, FBX helpers, etc.) are never allowed.
WINDOWS_SYSTEM_IMPORTS = frozenset("""
    advapi32.dll audioses.dll avrt.dll bcrypt.dll bcryptprimitives.dll cabinet.dll cfgmgr32.dll
    comctl32.dll comdlg32.dll combase.dll crypt32.dll cryptbase.dll cryptnet.dll
    cryptsp.dll d2d1.dll d3d9.dll d3d11.dll d3d12.dll d3dcompiler_47.dll dbgcore.dll dbghelp.dll
    dcomp.dll dhcpcsvc.dll dnsapi.dll dsound.dll dwmapi.dll dwrite.dll dxcore.dll
    dxgi.dll dxva2.dll faultrep.dll fwpuclnt.dll gdi32.dll gdiplus.dll glu32.dll
    hid.dll imagehlp.dll imm32.dll iphlpapi.dll kernel32.dll kernelbase.dll
    ksuser.dll mf.dll mfcore.dll mfplat.dll mfreadwrite.dll mfuuid.dll mmdevapi.dll
    mpr.dll msacm32.dll msasn1.dll msimg32.dll msvcrt.dll mswsock.dll netapi32.dll
    netutils.dll normaliz.dll nsi.dll ntdll.dll ole32.dll oleacc.dll oleaut32.dll
    opengl32.dll pdh.dll powrprof.dll propsys.dll psapi.dll rasapi32.dll rpcrt4.dll
    rsaenh.dll secur32.dll setupapi.dll shcore.dll shell32.dll shlwapi.dll sspicli.dll
    ucrtbase.dll urlmon.dll user32.dll userenv.dll usp10.dll uxtheme.dll version.dll
    wevtapi.dll win32u.dll windowscodecs.dll winhttp.dll wininet.dll winmm.dll
    winspool.drv wintrust.dll winusb.dll wldap32.dll ws2_32.dll wtsapi32.dll
""".split())


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


def check_vendored_backend_config(root: Path = ROOT) -> dict[str, str]:
    """Keep backend tests in the workspace and preserve an explicit upstream pin."""
    manifest = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    workspace = manifest.get("workspace", {})
    if BACKEND_PATH not in workspace.get("members", ()) or BACKEND_PATH in workspace.get("exclude", ()):
        raise ValueError("Vendored unity-rs-core must remain an explicit workspace member so its tests run")
    dependency = workspace.get("dependencies", {}).get("unity_rs_core", {})
    if not isinstance(dependency, dict) or dependency.get("package") != "unity-rs-core":
        raise ValueError("Expected the named unity_rs_core workspace dependency")
    upstream = dependency.get("git", "")
    revision = dependency.get("rev", "")
    if upstream != "https://github.com/seiunx-dev/unity-rs" or not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("Vendored backend requires its canonical upstream and exact 40-character revision")
    patch = manifest.get("patch", {}).get(upstream, {}).get("unity-rs-core", {})
    if not isinstance(patch, dict) or patch.get("path") != BACKEND_PATH:
        raise ValueError("Cargo must patch the pinned backend with the reviewed vendored source")
    return {"repository": upstream, "revision": revision}


def backend_provenance(package: dict, root: Path) -> dict:
    """Identify the actual compiled local reader, including binary decoder tables."""
    upstream = check_vendored_backend_config(root)
    backend = root / BACKEND_PATH
    if package.get("source") is not None or Path(package["manifest_path"]).resolve() != (backend / "Cargo.toml").resolve():
        raise ValueError("Cargo metadata must select the local vendored unity-rs-core source")
    for name in BACKEND_LOCAL_DOCUMENTS:
        require_file(backend / name)
    require_file(backend / "Cargo.toml")
    if (backend / "src").is_symlink() or not (backend / "src").is_dir():
        raise ValueError("Vendored backend source must be a regular directory")
    files = [backend / "Cargo.toml"]
    for path in (backend / "src").rglob("*"):
        if path.is_symlink():
            raise ValueError(f"Vendored backend source must not contain symbolic links: {path}")
        if path.is_file():
            files.append(path)
        if len(files) > 10_000:
            raise ValueError("Vendored backend source inventory exceeds 10,000 files")
    digest = hashlib.sha256()
    total_bytes = 0
    for path in sorted(files):
        if path.is_symlink() or not path.is_file():
            raise ValueError(f"Vendored backend source is not a regular file: {path}")
        total_bytes += path.stat().st_size
        if total_bytes > 512 * 1024 * 1024:
            raise ValueError("Vendored backend source inventory exceeds 512 MiB")
        name = path.relative_to(backend).as_posix().encode("utf-8")
        digest.update(len(name).to_bytes(4, "big"))
        digest.update(name)
        digest.update(bytes.fromhex(sha256(path)))
    return {
        "source": f"vendored+{upstream['repository']}#{upstream['revision']}",
        "upstream": upstream,
        "source_tree_sha256": digest.hexdigest(),
        "source_tree_files": len(files),
        "source_tree_bytes": total_bytes,
        "source_tree_scope": "Cargo.toml and every file under src/, with path-length/path/SHA256 framing",
        "local_modifications": "licenses/unity-rs-core/ASSETSTUDIO_PATCHES.md",
    }


def require_file(path: Path) -> None:
    if path.is_symlink() or not path.is_file() or path.stat().st_size == 0:
        raise ValueError(f"Missing, empty, or symbolic-link package input: {path}")


def copy_dependency_licenses(metadata: dict, stage: Path, root: Path = ROOT) -> list[dict]:
    """Retain the legal files supplied in every external Cargo package."""
    fallback_root = root / "packaging" / "licenses"
    fallback_manifest = fallback_root / "fallbacks.json"
    fallbacks = json.loads(fallback_manifest.read_text(encoding="utf-8")) if fallback_manifest.is_file() else {}
    inventory = []
    for package in metadata["packages"]:
        is_backend = package["name"] == "unity-rs-core"
        if not package.get("source") and not is_backend:
            continue
        provenance = backend_provenance(package, root) if is_backend else {"source": package["source"]}
        package_root = Path(package["manifest_path"]).parent
        identity = package["name"] + "-" + package["version"]
        source_suffix = hashlib.sha256(provenance["source"].encode("utf-8")).hexdigest()[:8]
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
            "license": package.get("license"), **provenance,
            "license_files": copied,
            "fallback_sources": fallback_sources,
        })
    return sorted(inventory, key=lambda package: (package["name"], package["version"], package["source"]))


def check_executable(path: Path) -> None:
    require_file(path)
    with path.open("rb") as source:
        size = path.stat().st_size
        if size < 64:
            raise ValueError(f"Not a Windows executable: {path}")

        def read_at(offset: int, length: int) -> bytes:
            if offset < 0 or length < 0 or offset + length > size:
                raise ValueError(f"Truncated PE range in {path}")
            source.seek(offset)
            value = source.read(length)
            if len(value) != length:
                raise ValueError(f"Truncated PE input: {path}")
            return value

        header = read_at(0, 64)
        if len(header) < 64 or header[:2] != b"MZ":
            raise ValueError(f"Not a Windows executable: {path}")
        pe_offset = int.from_bytes(header[60:64], "little")
        if not 64 <= pe_offset <= size - 24:
            raise ValueError(f"Invalid PE header offset: {path}")
        coff = read_at(pe_offset, 24)
        if coff[:4] != b"PE\0\0":
            raise ValueError(f"Missing PE signature: {path}")
        if coff[4:6] != b"\x64\x86":
            raise ValueError(f"Not a Windows x64 executable: {path}")
        sections = int.from_bytes(coff[6:8], "little")
        optional_size = int.from_bytes(coff[20:22], "little")
        if not 1 <= sections <= 96 or not 112 <= optional_size <= 4096:
            raise ValueError(f"Invalid PE section/optional-header count: {path}")
        optional = read_at(pe_offset + 24, optional_size)
        if optional[:2] != b"\x0b\x02":
            raise ValueError(f"Expected a PE32+ Windows x64 executable: {path}")
        header_size = struct.unpack_from("<I", optional, 60)[0]
        directory_count = struct.unpack_from("<I", optional, 108)[0]
        if not 14 <= directory_count <= 16 or optional_size < 112 + directory_count * 8:
            raise ValueError(f"PE import directories are missing or truncated: {path}")
        table_offset = pe_offset + 24 + optional_size
        if not table_offset + sections * 40 <= header_size <= size:
            raise ValueError(f"Invalid PE header size: {path}")
        ranges = []
        for index in range(sections):
            section = read_at(table_offset + index * 40, 40)
            virtual_size, virtual_start, raw_size, raw_start = struct.unpack_from("<IIII", section, 8)
            if raw_size and (raw_start < header_size or raw_start + raw_size > size):
                raise ValueError(f"PE section leaves its input file: {path}")
            virtual_end = virtual_start + max(virtual_size, raw_size)
            if virtual_end > 0x1_0000_0000:
                raise ValueError(f"PE virtual section range overflows: {path}")
            if virtual_start < header_size or any(virtual_start < end and start < virtual_end for start, end, _, _ in ranges):
                raise ValueError(f"PE virtual sections overlap: {path}")
            ranges.append((virtual_start, virtual_end, raw_start, raw_size))

        def read_rva(address: int, length: int) -> bytes:
            if address < header_size and address + length <= header_size:
                return read_at(address, length)
            for start, end, raw_start, raw_size in ranges:
                if start <= address < end and address - start + length <= raw_size:
                    return read_at(raw_start + address - start, length)
            raise ValueError(f"Unmapped or truncated PE import address in {path}")

        def dll_name(address: int) -> str:
            value = bytearray()
            for index in range(256):
                byte = read_rva(address + index, 1)[0]
                if not byte:
                    try:
                        name = value.decode("ascii")
                    except UnicodeDecodeError as error:
                        raise ValueError(f"Non-ASCII PE import name in {path}") from error
                    if not re.fullmatch(r"[A-Za-z0-9_.-]+", name):
                        raise ValueError(f"Unsafe PE import name in {path}")
                    return name
                value.append(byte)
            raise ValueError(f"PE import name exceeds 255 bytes: {path}")

        imports = set()
        for directory, record_size in ((1, 20), (13, 32)):
            address, length = struct.unpack_from("<II", optional, 112 + directory * 8)
            if address == 0 and length == 0:
                continue
            if not address or length < record_size:
                raise ValueError(f"Invalid PE import directory: {path}")
            terminated = False
            for index in range(min(length // record_size, 513)):
                descriptor = read_rva(address + index * record_size, record_size)
                if not any(descriptor):
                    terminated = True
                    break
                if index == 512:
                    raise ValueError(f"PE imports exceed 512 DLLs: {path}")
                if directory == 1:
                    name_address = struct.unpack_from("<I", descriptor, 12)[0]
                else:
                    attributes, name_address = struct.unpack_from("<II", descriptor)
                    if attributes != 1:
                        raise ValueError(f"Unsupported PE delayed-import address format: {path}")
                if name_address == 0:
                    raise ValueError(f"PE import has no DLL name: {path}")
                imports.add(dll_name(name_address))
            if not terminated:
                raise ValueError(f"PE import directory has no terminator: {path}")
        for name in sorted(imports):
            lower = name.lower()
            api_set = re.fullmatch(r"(?:api|ext)-ms-win-[a-z0-9-]+\.dll", lower)
            if lower not in WINDOWS_SYSTEM_IMPORTS and not api_set:
                raise ValueError(f"Portable Windows executable imports a non-system runtime DLL: {name} in {path}; MSVC builds require +crt-static")


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
        required.update(PACKAGE_RESOURCES)
        required.update(VENDORED_LICENSES)
        required.update("licenses/unity-rs-core/" + name for name in BACKEND_DOCUMENTS + BACKEND_LOCAL_DOCUMENTS)
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
    for source in (*PACKAGE_RESOURCES.values(), *VENDORED_LICENSES.values()):
        require_file(root / source)
    metadata = metadata if metadata is not None else cargo_metadata(root)
    backend = backend_directory(metadata)
    backend_package = next(package for package in metadata["packages"] if package["name"] == "unity-rs-core")
    backend_provenance(backend_package, root)
    for name in BACKEND_DOCUMENTS + BACKEND_LOCAL_DOCUMENTS:
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
        for name, source in PACKAGE_RESOURCES.items():
            shutil.copy2(root / source, stage / name)
        for name, source in VENDORED_LICENSES.items():
            target = stage / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(root / source, target)
        licenses = stage / "licenses" / "unity-rs-core"
        licenses.mkdir(parents=True)
        for name in BACKEND_DOCUMENTS + BACKEND_LOCAL_DOCUMENTS:
            shutil.copy2(backend / name, licenses / name)
        for name in (
            "docs/rust-rewrite.md", "docs/feature-parity.md",
            "docs/game-compatibility.md", "docs/review-sweeps.md", "docs/releasing.md",
            "docs/reviews/sweep2.md", "docs/reviews/sweep2-source-ledger.json",
            "crates/assetstudio-cli/README.md",
        ):
            document = root / name
            if document.is_file():
                require_file(document)
                destination = stage / name
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(document, destination)
        dependencies = copy_dependency_licenses(metadata, stage, root)
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
        # Cargo registry archives normalize source mtimes to dates that can
        # precede ZIP's 1980 minimum. Preserve legal bytes while clamping only
        # the ZIP metadata timestamp to its representable range.
        with zipfile.ZipFile(temporary_zip, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9, strict_timestamps=False) as archive:
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
