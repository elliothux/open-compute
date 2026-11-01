#!/usr/bin/env python3
"""Classify changed files so CI can skip unrelated heavyweight checks."""

import argparse
from fnmatch import fnmatchcase
import json
import subprocess

from gate import CARGO_TARGETS, selection


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

# Unknown/shared inputs select all product Gates, not workspace unit or package tests.
PRODUCT_TARGETS = tuple(name for name in selection(['all']) if name != 'single-binary')
PRODUCT_PATHS = {
    ('crates/*/src/kv*', 'crates/*/src/cloudflare_v4/kv*'): ('kv',),
    ('crates/*/src/r2*', 'crates/*/src/cloudflare_v4/r2*'): ('r2',),
    ('crates/*/src/d1*', 'crates/*/src/cloudflare_v4/d1*'): ('d1',),
    ('crates/*/src/durable_objects*', 'crates/*/src/do_*'): ('durable-objects', 'alarms'),
    ('crates/*/src/queue*', 'crates/*/src/cloudflare_v4/queues*',
     'crates/*/src/scheduler/queue*'): ('workers', 'scheduler', 'queues', 'queue-workflow-recovery'),
    ('crates/*/src/workflow*', 'crates/*/src/cloudflare_v4/workflows*',
     'crates/*/src/scheduler/workflow*'): ('workflow',),
    ('crates/*/src/services*', 'crates/*/src/service_binding*'): ('services',),
    ('crates/*/src/cache*', 'crates/*/src/image*', 'crates/*/src/workers_cache*',
     'crates/*/src/cloudflare_v4/images*'): ('cache-images',),
    ('crates/*/src/asset*', 'crates/*/src/workers_http/v4/asset*'): ('assets',),
    ('crates/search/*', 'crates/*/src/ai_search*',
     'crates/*/src/cloudflare_v4/ai_search*'): ('search',),
    ('crates/*/src/*container*', 'crates/*/src/cloudflare_v4/container*',
     'test/applications/containers/*', 'test/containers-fixture/*'): ('containers',),
    ('crates/*/src/browser*', 'test/applications/browser-run/*',
     'test/browser-fixture.lock.json', 'scripts/prepare-browser.ts'): ('browser',),
    ('apps/dashboard/*',): ('dashboard',),
    ('docs/references/p1-deviations.md', 'test/conformance/*', 'openapi/*'): ('contract',),
}


def affected_targets(paths: list[str]) -> list[str]:
    if not paths:
        return list(PRODUCT_TARGETS)
    targets = set()
    for path in paths:
        matched = False
        for patterns, names in PRODUCT_PATHS.items():
            if any(fnmatchcase(path, pattern) for pattern in patterns):
                targets.update(selection(names))
                matched = True
        for name, (_, native, _) in CARGO_TARGETS.items():
            owned = (native, native.removesuffix('_gate') + '_support')
            if any(fnmatchcase(path, f'crates/*/tests/{stem}.rs') or
                   fnmatchcase(path, f'crates/*/tests/{stem}/*') for stem in owned):
                targets.add(name)
                matched = True
            if name.startswith('python-') and any(path.startswith(root) for root in (
                    f'test/applications/{name}/',
                    f'test/fixtures/{name}/')):
                targets.add(name)
                matched = True
        if matched:
            continue
        if path.startswith('crates/service/tests/python_support/'):
            targets.update(name for name in CARGO_TARGETS if name.startswith('python-'))
        elif path.startswith('docs/references/runbooks/'):
            return list(PRODUCT_TARGETS)
        elif (is_docs_path(path) or path.startswith('apps/website/') or path in RELEASE_TOOLING):
            continue
        else:
            return list(PRODUCT_TARGETS)
    if targets:
        targets.update(('contract', 'recorded'))
    return [name for name in PRODUCT_TARGETS if name in targets]


def git_output(args: list[str]) -> str:
    return subprocess.check_output(
        ["git", *args], text=True, stderr=subprocess.DEVNULL
    ).strip()


def diff_files(base: str, head: str) -> list[str]:
    return git_output(
        ["diff", "--name-only", "--no-renames", base, head]
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
    return (path.startswith(("docs/", ".agents/", ".claude/"))
            or path.rsplit("/", 1)[-1] in {"AGENTS.md", "CLAUDE.md"}
            or path in {"README.md", "README.zh.md"})


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
    print(f"gate_targets={' '.join(affected_targets(paths))}")


if __name__ == "__main__":
    main()
