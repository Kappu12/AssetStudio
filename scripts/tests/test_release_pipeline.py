"""Exercise packaging and publication with generated fixtures and a fake CLI.

No tests contact GitHub or create remote releases. The fake CLI stores uploaded
bytes so the real publisher must complete the same download/hash verification.
"""

from __future__ import annotations

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import zipfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from check_workflow_policy import ROOT as SOURCE_ROOT, check_policy
from package_release import BACKEND_DOCUMENTS, BINARIES, DOCUMENTS, inspect_zip, make_package, sha256, verify_tag, workspace_version
from publish_release import guard_tag_push, publish


def fake_pe() -> bytes:
    data = bytearray(96)
    data[:2] = b"MZ"
    data[60:64] = (64).to_bytes(4, "little")
    data[64:68] = b"PE\0\0"
    data[68:70] = b"\x64\x86"  # IMAGE_FILE_MACHINE_AMD64
    return bytes(data)


class Fixture(unittest.TestCase):
    version = "1.2.3-alpha.1"

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.build = self.root / "target" / "release"
        self.build.mkdir(parents=True)
        self.backend = self.root / "backend"
        self.backend.mkdir()
        self.codec = self.root / "codec"
        self.codec.mkdir()
        (self.codec / "LICENSE-MIT").write_text("Fixture codec MIT notice\n", encoding="utf-8")
        self.set_version(self.version)
        for name in BINARIES:
            (self.build / name).write_bytes(fake_pe())
        for name in DOCUMENTS:
            (self.root / name).write_text(f"Fixture document {name}\n", encoding="utf-8")
        for name in BACKEND_DOCUMENTS:
            (self.backend / name).write_text(f"Backend legal document {name}\n", encoding="utf-8")
        self.metadata = {"packages": [
            {"name": "unity-rs-core", "version": "0.6.1", "manifest_path": str(self.backend / "Cargo.toml"),
             "license": "MIT", "source": "git+https://github.com/seiunx-dev/unity-rs#fixture"},
            {"name": "fixture-codec", "version": "1.0.0", "manifest_path": str(self.codec / "Cargo.toml"),
             "license": "MIT OR Apache-2.0", "source": "registry+fixture"},
        ]}

    def set_version(self, value: str) -> None:
        (self.root / "Cargo.toml").write_text(f'[workspace.package]\nversion = "{value}"\n', encoding="utf-8")

    def package(self) -> tuple[Path, Path]:
        return make_package(self.build, self.root / "dist", root=self.root, metadata=self.metadata)

    def environment(self) -> dict[str, str]:
        tag = "v" + workspace_version(self.root)
        return {"GITHUB_EVENT_NAME": "push", "GITHUB_REF_TYPE": "tag", "GITHUB_REF": f"refs/tags/{tag}", "GITHUB_REF_NAME": tag}


class PackageTests(Fixture):
    def test_package_contains_binaries_notices_and_verified_manifest(self) -> None:
        archive, checksum = self.package()
        inspect_zip(archive, self.version)
        self.assertEqual(checksum.read_text(), f"{sha256(archive)}  {archive.name}\n")
        with zipfile.ZipFile(archive) as package:
            names = set(package.namelist())
            self.assertTrue(set(BINARIES + DOCUMENTS).issubset(names))
            inventory = json.loads(package.read("dependency-inventory.json"))
            self.assertEqual([p["name"] for p in inventory], ["fixture-codec", "unity-rs-core"])
            self.assertEqual(package.read(inventory[0]["license_files"][0]), b"Fixture codec MIT notice\n")
        self.assertEqual(sorted(p.name for p in archive.parent.iterdir()), [archive.name, checksum.name])

    def test_missing_executable_prevents_package(self) -> None:
        (self.build / "AssetStudio.exe").unlink()
        with self.assertRaisesRegex(ValueError, "package input"):
            self.package()
        self.assertFalse((self.root / "dist").exists())

    def test_text_masquerading_as_executable_is_rejected(self) -> None:
        (self.build / "AssetStudio.exe").write_text("not an executable")
        with self.assertRaisesRegex(ValueError, "Windows executable"):
            self.package()

    def test_other_architecture_cannot_be_mislabeled_as_win_x64(self) -> None:
        data = bytearray(fake_pe())
        data[68:70] = b"\x4c\x01"  # IMAGE_FILE_MACHINE_I386
        (self.build / "AssetStudio.exe").write_bytes(data)
        with self.assertRaisesRegex(ValueError, "x64 executable"):
            self.package()

    def test_missing_backend_license_prevents_package(self) -> None:
        (self.backend / "LICENSE").unlink()
        with self.assertRaisesRegex(ValueError, "package input"):
            self.package()

    def test_tag_mismatch_fails_before_staging(self) -> None:
        with self.assertRaisesRegex(ValueError, "must equal workspace"):
            make_package(self.build, self.root / "dist", root=self.root, tag="v9.9.9", metadata=self.metadata)
        self.assertFalse((self.root / "dist").exists())

    def test_invalid_semver_and_numeric_prerelease_are_rejected(self) -> None:
        for version in ("01.2.3", "1.2", "1.2.3/../../oops", "1.2.3-01"):
            self.set_version(version)
            with self.subTest(version=version), self.assertRaisesRegex(ValueError, "SemVer"):
                workspace_version(self.root)

    def test_member_tampering_fails_manifest_hash_check(self) -> None:
        archive, _ = self.package()
        with zipfile.ZipFile(archive) as source:
            entries = {name: source.read(name) for name in source.namelist()}
        entries["AssetStudioCLI.exe"] = b"different executable bytes"
        with zipfile.ZipFile(archive, "w") as target:
            for name, data in entries.items():
                target.writestr(name, data)
        with self.assertRaisesRegex(ValueError, "hash mismatch"):
            inspect_zip(archive, self.version)

    def test_archive_traversal_is_rejected(self) -> None:
        archive, _ = self.package()
        with zipfile.ZipFile(archive, "a") as target:
            target.writestr("../outside.txt", b"outside")
        with self.assertRaisesRegex(ValueError, "Unsafe ZIP"):
            inspect_zip(archive, self.version)

    def test_case_colliding_archive_members_are_rejected(self) -> None:
        archive, _ = self.package()
        with zipfile.ZipFile(archive, "a") as target:
            target.writestr("assetstudio.exe", b"collision")
        with self.assertRaisesRegex(ValueError, "case-colliding"):
            inspect_zip(archive, self.version)

    def test_old_native_helper_is_not_allowed_in_package(self) -> None:
        archive, _ = self.package()
        with zipfile.ZipFile(archive, "a") as target:
            target.writestr("AssetStudio.FBXNative.dll", b"old helper")
        with self.assertRaisesRegex(ValueError, "native/managed"):
            inspect_zip(archive, self.version)

    def test_symbolic_link_input_is_rejected(self) -> None:
        binary = self.build / "AssetStudio.exe"
        other = self.build / "source.exe"
        binary.replace(other)
        try:
            binary.symlink_to(other)
        except OSError:
            self.skipTest("Creating symbolic links is unavailable on this runner")
        with self.assertRaisesRegex(ValueError, "symbolic-link"):
            self.package()


class FakeGitHub:
    def __init__(self, *, exists: bool = False, draft: bool = True, corrupt_download: bool = False, bad_asset_size: bool = False):
        self.exists = exists
        self.draft = draft
        self.calls = []
        self.uploaded = {}
        self.corrupt_download = corrupt_download
        self.bad_asset_size = bad_asset_size
        self.tag = None

    def __call__(self, arguments: list[str]) -> subprocess.CompletedProcess[str]:
        self.calls.append(arguments)
        command = arguments[1]
        self.tag = arguments[2]
        value = {}
        if command == "view":
            if not self.exists:
                raise subprocess.CalledProcessError(1, ["gh", *arguments], stderr="not found")
            value = {"isDraft": self.draft, "tagName": self.tag, "assets": [
                {"name": name, "state": "uploaded", "size": len(data) + int(self.bad_asset_size)}
                for name, data in self.uploaded.items()
            ]}
        elif command == "create":
            self.exists = True
            self.draft = True
        elif command == "upload":
            for path in arguments[3:-1]:
                item = Path(path)
                self.uploaded[item.name] = item.read_bytes()
        elif command == "download":
            directory = Path(arguments[arguments.index("--dir") + 1])
            for name, data in self.uploaded.items():
                (directory / name).write_bytes(data + (b"corrupt" if self.corrupt_download else b""))
        elif command == "edit":
            self.draft = False
        else:
            raise AssertionError(f"Unexpected gh operation: {arguments}")
        return subprocess.CompletedProcess(["gh", *arguments], 0, stdout=json.dumps(value), stderr="")


class PublishTests(Fixture):
    def publish(self, gh: FakeGitHub, *, environment: dict[str, str] | None = None) -> None:
        archive, checksum = self.package()
        publish("v" + workspace_version(self.root), archive, checksum, root=self.root,
                environment=environment if environment is not None else self.environment(), gh=gh)

    def test_branch_and_manual_runs_cannot_publish(self) -> None:
        for overrides in ({"GITHUB_REF_TYPE": "branch"}, {"GITHUB_EVENT_NAME": "workflow_dispatch"}, {"GITHUB_REF_NAME": "v0.0.0"}):
            environment = self.environment() | overrides
            gh = FakeGitHub()
            with self.subTest(overrides=overrides), self.assertRaises(ValueError):
                self.publish(gh, environment=environment)
            self.assertEqual(gh.calls, [])

    def test_matching_tag_guard_accepts_only_workspace_version(self) -> None:
        self.assertEqual(guard_tag_push("v" + self.version, self.environment(), self.root), self.version)
        with self.assertRaises(ValueError):
            verify_tag("v9.9.9", self.root)

    def test_published_release_cannot_be_overwritten(self) -> None:
        gh = FakeGitHub(exists=True, draft=False)
        with self.assertRaisesRegex(ValueError, "already published"):
            self.publish(gh)
        self.assertEqual([call[1] for call in gh.calls], ["view"])

    def test_upload_and_download_verification_precede_publication(self) -> None:
        gh = FakeGitHub()
        self.publish(gh)
        self.assertFalse(gh.draft)
        operations = [call[1] for call in gh.calls]
        self.assertEqual(operations, ["view", "create", "upload", "view", "download", "edit"])
        create = next(call for call in gh.calls if call[1] == "create")
        self.assertIn("--draft", create)
        self.assertIn("--verify-tag", create)
        self.assertIn("--prerelease", create)
        self.assertIn("--latest=false", gh.calls[-1])

    def test_failed_upload_verification_leaves_draft(self) -> None:
        gh = FakeGitHub(bad_asset_size=True)
        with self.assertRaisesRegex(ValueError, "wrong size"):
            self.publish(gh)
        self.assertTrue(gh.draft)
        self.assertNotIn("edit", [call[1] for call in gh.calls])

    def test_corrupted_download_leaves_draft(self) -> None:
        gh = FakeGitHub(corrupt_download=True)
        with self.assertRaisesRegex(ValueError, "download verification"):
            self.publish(gh)
        self.assertTrue(gh.draft)
        self.assertNotIn("edit", [call[1] for call in gh.calls])

    def test_retry_resumes_existing_draft(self) -> None:
        gh = FakeGitHub(exists=True, draft=True)
        self.publish(gh)
        self.assertNotIn("create", [call[1] for call in gh.calls])
        self.assertFalse(gh.draft)

    def test_stable_tag_does_not_become_prerelease(self) -> None:
        self.set_version("1.2.3")
        gh = FakeGitHub()
        self.publish(gh)
        self.assertIn("--prerelease=false", gh.calls[-1])
        self.assertIn("--latest=true", gh.calls[-1])

    def test_bad_checksum_aborts_before_any_remote_operation(self) -> None:
        archive, checksum = self.package()
        checksum.write_text("wrong checksum\n")
        gh = FakeGitHub()
        with self.assertRaisesRegex(ValueError, "checksum"):
            publish("v" + self.version, archive, checksum, root=self.root, environment=self.environment(), gh=gh)
        self.assertEqual(gh.calls, [])


class WorkflowPolicyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.workflow = (SOURCE_ROOT / ".github/workflows/build.yml").read_text(encoding="utf-8")

    def test_workflow_has_read_only_ci_and_no_persistent_storage(self) -> None:
        self.assertEqual(check_policy(self.workflow), [])

    def test_artifact_and_cache_actions_are_rejected(self) -> None:
        for action in ("actions/upload-artifact@v4", "actions/cache@v4", "Swatinem/rust-cache@v2"):
            with self.subTest(action=action):
                self.assertTrue(check_policy(self.workflow + f"\n      - uses: {action}\n"))

    def test_missing_tag_guard_is_rejected(self) -> None:
        unsafe = self.workflow.replace("github.event_name == 'push' && github.ref_type == 'tag' && ", "")
        self.assertTrue(check_policy(unsafe))

    def test_global_write_token_is_rejected(self) -> None:
        unsafe = self.workflow.replace("permissions:\n  contents: read", "permissions:\n  contents: write")
        self.assertTrue(check_policy(unsafe))


if __name__ == "__main__":
    unittest.main()
