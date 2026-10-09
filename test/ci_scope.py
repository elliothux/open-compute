#!/usr/bin/env python3
"""Classify changed files so CI can skip unrelated heavyweight checks."""

import argparse
import json
import subprocess


FRONTEND_ROOTS = {
    "sdk": "packages/sdk/",
    "dashboard": "apps/dashboard/",
    "website": "apps/website/",
}

RELEASE_TOOLING = {
    ".github/workflows/release.yml",
    ".github/workflows/release-recovery.yml",
    "scripts/assemble-release.ts",
    "scripts/release-test-report.ts",
    "scripts/release-dry-run.sh",
    "test/release-dry-run/Dockerfile",
    "test/release-tools.test.mjs",
    "test/release-test-report.test.mjs",
    "test/fixtures/release-evidence.mjs",
}

BASELINE_PATH = "test/conformance/baseline.json"


def git_output(args: list[str]) -> str:
    return subprocess.check_output(
        ["git", *args], text=True, stderr=subprocess.DEVNULL
    ).strip()


def diff_files(base: str, head: str) -> list[str]:
    return git_output(
        ["diff", "--name-only", "--diff-filter=ACMRTUXB", base, head]
    ).splitlines()


def source_digest_only_changed(base: str, head: str) -> bool:
    try:
        before = json.loads(git_output(["show", f"{base}:{BASELINE_PATH}"]))
        after = json.loads(git_output(["show", f"{head}:{BASELINE_PATH}"]))
    except (json.JSONDecodeError, subprocess.CalledProcessError):
        return False
    if not isinstance(before, dict) or not isinstance(after, dict):
        return False
    before_digest = before.pop("sourceDigest", None)
    after_digest = after.pop("sourceDigest", None)
    return (
        isinstance(before_digest, str)
        and isinstance(after_digest, str)
        and before_digest != after_digest
        and before == after
    )


def previous_baseline_revision(head: str) -> str | None:
    revisions = git_output(
        ["log", "--format=%H", "-2", head, "--", BASELINE_PATH]
    ).splitlines()
    if len(revisions) != 2 or revisions[0] != git_output(["rev-parse", head]):
        return None
    return revisions[1]


def changed_files(base: str, head: str) -> list[str]:
    if not base or set(base) == {"0"}:
        base = git_output(["rev-parse", "HEAD^"])
    paths = diff_files(base, head)
    if BASELINE_PATH not in paths or not source_digest_only_changed(base, head):
        return paths
    if paths == [BASELINE_PATH]:
        previous = previous_baseline_revision(head)
        if previous is None:
            return paths
        paths = diff_files(previous, head)
    # Only sourceDigest is a neutral companion to its owning files.
    return [path for path in paths if path != BASELINE_PATH]


def is_docs_path(path: str) -> bool:
    return path.startswith("docs/") or path in {"README.md", "README.zh.md"}


def classify(paths: list[str]) -> tuple[str, str]:
    if not paths:
        return "full", ""
    docs_changed = any(is_docs_path(path) for path in paths)
    owned_paths = [path for path in paths if not is_docs_path(path)]
    if not owned_paths:
        return "docs", ""

    if all(path in RELEASE_TOOLING for path in owned_paths):
        return "release-tooling", ""

    components = {"docs"} if docs_changed else set()
    for path in owned_paths:
        if path == "bun.lock" or path == "package.json":
            components.update(FRONTEND_ROOTS)
            continue
        match = next(
            (name for name, root in FRONTEND_ROOTS.items() if path.startswith(root)),
            None,
        )
        if match is None:
            return "full", ""
        components.add(match)
    return "frontend", ",".join(sorted(components))


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", required=True)
    parser.add_argument("--head", required=True)
    args = parser.parse_args()
    paths = changed_files(args.base, args.head)
    scope, components = classify(paths)
    print(f"scope={scope}")
    print(f"components={components}")
    print(f"changed_files={len(paths)}")


if __name__ == "__main__":
    main()
