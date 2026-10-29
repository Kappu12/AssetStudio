#!/usr/bin/env python3
"""Enforce this repository's no-artifact/cache release workflow policy."""

from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
ALLOWED_ACTIONS = {"actions/checkout", "actions/setup-python"}


def check_policy(text: str) -> list[str]:
    errors = []
    for match in re.finditer(r"^\s*(?:-\s+)?uses:\s*([^\s#]+)", text, flags=re.MULTILINE):
        action = match[1].strip("\"'")
        name, separator, reference = action.partition("@")
        if name not in ALLOWED_ACTIONS or not separator or not re.fullmatch(r"[0-9a-f]{40}", reference):
            errors.append(f"Action must be an approved, commit-pinned checkout/setup step: {action}")
    for expression in (r"gh\s+cache\b", r"/actions/artifacts\b", r"/actions/caches\b", r"ACTIONS_(?:CACHE|RESULTS)_URL"):
        if re.search(expression, text, re.IGNORECASE):
            errors.append("Workflow must not use persistent Actions artifact/cache storage")
    guard = "if: github.event_name == 'push' && github.ref_type == 'tag' && startsWith(github.ref, 'refs/tags/v')"
    release = re.search(r"^  release:\n(.*?)(?=^  [a-zA-Z][\w-]*:\n|\Z)", text, re.MULTILINE | re.DOTALL)
    if not release or guard not in release[1] or "    needs: validate" not in release[1]:
        errors.append("Release job must require successful validation and an explicit v-tag push")
    if not re.search(r"^permissions:\n  contents: read\s*$", text, re.MULTILINE):
        errors.append("Top-level token must have contents: read")
    if text.count("contents: write") != 1 or not release or "contents: write" not in release[1]:
        errors.append("Only the tag-gated release job may have contents: write")
    for runner in ("ubuntu-24.04", "windows-latest", "macos-latest"):
        if runner not in text:
            errors.append(f"Missing GitHub-hosted build/test platform: {runner}")
    if "--locked" not in text or "cargo test --workspace --all-targets --locked" not in text:
        errors.append("Workflow must test all workspace targets against Cargo.lock")
    return errors


def main() -> None:
    workflows = sorted((ROOT / ".github/workflows").glob("*.yml")) + sorted((ROOT / ".github/workflows").glob("*.yaml"))
    if len(workflows) != 1:
        raise SystemExit("Expected the single audited Rust build/release workflow")
    errors = check_policy(workflows[0].read_text(encoding="utf-8"))
    if errors:
        raise SystemExit("\n".join(errors))
    print("Workflow policy verified: tag-only releases, read-only CI, no Actions artifacts/caches")


if __name__ == "__main__":
    main()
