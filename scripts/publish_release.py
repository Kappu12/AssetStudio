#!/usr/bin/env python3
"""Publish a verified ZIP directly to GitHub Releases after a tag push.

An upload or verification failure leaves a draft, never a published release
with incomplete assets. Existing published releases are never modified.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
from typing import Callable

from package_release import ROOT, inspect_zip, sha256, verify_tag


def run_gh(arguments: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(["gh", *arguments], capture_output=True, text=True, encoding="utf-8", check=True)


def guard_tag_push(tag: str, environment: dict[str, str], root: Path = ROOT) -> str:
    if environment.get("GITHUB_EVENT_NAME") != "push" or environment.get("GITHUB_REF_TYPE") != "tag":
        raise ValueError("Publishing is allowed only for an explicit tag-push event")
    if environment.get("GITHUB_REF") != f"refs/tags/{tag}" or environment.get("GITHUB_REF_NAME") != tag:
        raise ValueError("Requested release tag is not the event's pushed tag")
    return verify_tag(tag, root)


def publish(
    tag: str,
    archive: Path,
    checksum: Path,
    *,
    root: Path = ROOT,
    environment: dict[str, str] | None = None,
    gh: Callable[[list[str]], subprocess.CompletedProcess[str]] = run_gh,
) -> None:
    environment = environment if environment is not None else dict(os.environ)
    version = guard_tag_push(tag, environment, root)
    expected_name = f"AssetStudio-win-x64-{tag}.zip"
    if archive.name != expected_name or checksum.name != f"{expected_name}.sha256":
        raise ValueError("Release asset names do not match the pushed tag")
    inspect_zip(archive, version)
    expected_checksum = f"{sha256(archive)}  {archive.name}\n"
    if checksum.read_text(encoding="utf-8") != expected_checksum:
        raise ValueError("Release checksum does not match the verified ZIP")
    try:
        state = json.loads(gh(["release", "view", tag, "--json", "isDraft,tagName"]).stdout)
    except subprocess.CalledProcessError:
        state = None
    if state is not None:
        if not state.get("isDraft") or state.get("tagName") != tag:
            raise ValueError("Refusing to modify an already published or mismatched release")
    else:
        prerelease = "-" in version.split("+", 1)[0]
        arguments = ["release", "create", tag, "--draft", "--verify-tag", "--title", f"AssetStudio {tag}", "--generate-notes"]
        if prerelease:
            arguments.extend(["--prerelease", "--latest=false"])
        gh(arguments)
    gh(["release", "upload", tag, str(archive), str(checksum), "--clobber"])
    state = json.loads(gh(["release", "view", tag, "--json", "isDraft,assets"]).stdout)
    if not state.get("isDraft"):
        raise ValueError("Release ceased to be a draft during upload; refusing to publish")
    uploaded = {asset["name"]: asset for asset in state.get("assets", [])}
    for path in (archive, checksum):
        remote = uploaded.get(path.name)
        if remote is None or remote.get("size") != path.stat().st_size or remote.get("state") != "uploaded":
            raise ValueError(f"Release asset is missing, incomplete, or has the wrong size: {path.name}")
    with tempfile.TemporaryDirectory(prefix="assetstudio-release-check-") as directory:
        gh(["release", "download", tag, "--dir", directory, "--pattern", archive.name, "--pattern", checksum.name])
        for local in (archive, checksum):
            downloaded = Path(directory) / local.name
            if not downloaded.is_file() or sha256(downloaded) != sha256(local):
                raise ValueError(f"Uploaded release asset failed download verification: {local.name}")
    prerelease = "-" in version.split("+", 1)[0]
    gh(["release", "edit", tag, "--draft=false", f"--prerelease={str(prerelease).lower()}", f"--latest={str(not prerelease).lower()}"])


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--zip", dest="archive", type=Path, required=True)
    parser.add_argument("--checksum", type=Path, required=True)
    args = parser.parse_args()
    publish(args.tag, args.archive, args.checksum)


if __name__ == "__main__":
    main()
