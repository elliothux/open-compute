#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

PYTHONDONTWRITEBYTECODE=1 python3 - <<'PY'
from pathlib import Path
import os
import re
import subprocess
import sys

errors: list[str] = []
maintained = subprocess.check_output(
    ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"]
).decode().split("\0")
deleted = set(
    subprocess.check_output(
        ["git", "diff", "--name-only", "--diff-filter=D", "-z"]
    ).decode().split("\0")
)
untracked = {
    path
    for path in subprocess.check_output(
        ["git", "ls-files", "--others", "--exclude-standard", "-z"]
    ).decode().split("\0")
    if path
}
untracked_casefold = {path.casefold(): path for path in untracked}

def superseded_case_path(raw: str) -> bool:
    replacement = untracked_casefold.get(raw.casefold())
    return replacement is not None and replacement != raw

def exact_path_exists(raw: str) -> bool:
    current = Path(".")
    for part in Path(raw).parts:
        try:
            names = {entry.name for entry in os.scandir(current)}
        except OSError:
            return False
        if part not in names:
            return False
        current /= part
    return current.exists()

def is_rust_test_source(raw: str, path: Path) -> bool:
    parts = path.parts
    return (
        raw.startswith("test/")
        or "tests" in parts
        or path.name == "tests.rs"
        or path.name.endswith("_tests.rs")
        or path.name.endswith("_fixture.rs")
    )

for raw in maintained:
    if not raw or raw in deleted or superseded_case_path(raw):
        continue
    if not exact_path_exists(raw):
        errors.append(f"{raw}: tracked path casing does not match the filesystem")
        continue
    path = Path(raw)
    if not path.is_file():
        continue
    if raw.startswith(("crates/", "test/")) and path.suffix == ".rs":
        lines = len(path.read_bytes().splitlines())
        limit = 2000 if is_rust_test_source(raw, path) else 800
        if lines > limit:
            kind = "test" if limit == 2000 else "production"
            errors.append(
                f"{raw}: {lines} physical lines exceeds the Rust {kind} source limit of {limit}"
            )
        text = path.read_text(errors="replace")
        if re.search(r"#\[(?:allow|expect)\([^\]]*clippy::too_many_lines", text):
            errors.append(f"{raw}: local clippy::too_many_lines suppression is forbidden")

    if raw.startswith("apps/dashboard/"):
        for part in path.parts[2:]:
            if part.startswith("$"):
                # TanStack Router requires dynamic parameter names to be JavaScript identifiers.
                stem = part.split(".", 1)[0][1:]
                if not re.fullmatch(r"[A-Za-z_$][A-Za-z0-9_$]*", stem):
                    errors.append(f"{raw}: invalid TanStack Router parameter filename")
                continue
            stem = part.split(".", 1)[0].lstrip("_")
            if stem and not re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", stem):
                errors.append(f"{raw}: maintained Dashboard paths must use lowercase kebab-case")

for raw in maintained:
    if not raw or raw in deleted or superseded_case_path(raw) or not exact_path_exists(raw):
        continue
    if not Path(raw).is_file():
        continue
    if re.search(r"(?:^|/)(__pycache__|dist|\.next|\.wrangler|target)(?:/|$)", raw):
        errors.append(f"{raw}: generated output must not be tracked")
    if raw.endswith((".pyc", ".profraw")):
        errors.append(f"{raw}: generated runtime output must not be tracked")
    if raw.endswith(("Cargo.lock", "bun.lock")) and raw not in {"Cargo.lock", "bun.lock"}:
        errors.append(f"{raw}: only root lockfiles are permitted")

for pattern in ("crates/**/*.profraw", "packages/**/*.profraw", "test/**/*.profraw"):
    for path in Path(".").glob(pattern):
        errors.append(f"{path}: coverage profiles belong under target/llvm-cov-target or .temp/gate-run")

if errors:
    print("\n".join(errors), file=sys.stderr)
    sys.exit(1)
print("source policy ok")
PY
