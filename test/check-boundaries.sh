#!/bin/sh
# Workspace crate dependency boundary check from `cargo metadata`.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
./test/check-source-policy.sh
cargo metadata --format-version 1 --no-deps --offline >/dev/null
python3 - <<'PY'
import json, subprocess, sys
meta = json.loads(subprocess.check_output(["cargo", "metadata", "--format-version", "1", "--no-deps"]))
members = {p["name"]: p for p in meta["packages"] if p["id"] in set(meta["workspace_members"]) or p["name"].startswith("open-compute-")}
# cargo metadata workspace_members may be ids
ids = set(meta["workspace_members"])
pkgs = [p for p in meta["packages"] if p["id"] in ids]
by = {p["name"]: p for p in pkgs}
forbidden = {
    "open-compute-document-parser": {"open-compute-core", "open-compute-search", "open-compute-images", "open-compute-storage", "open-compute-artifacts", "open-compute-runtime", "open-compute-workers", "open-compute-service"},
    "open-compute-core": {"open-compute-search", "open-compute-images", "open-compute-storage", "open-compute-artifacts", "open-compute-runtime", "open-compute-workers", "open-compute-service"},
    "open-compute-search": {"open-compute-images", "open-compute-storage", "open-compute-artifacts", "open-compute-runtime", "open-compute-workers", "open-compute-service"},
    "open-compute-images": {"open-compute-search", "open-compute-storage", "open-compute-artifacts", "open-compute-runtime", "open-compute-workers", "open-compute-service"},
    "open-compute-storage": {"open-compute-images", "open-compute-artifacts", "open-compute-runtime", "open-compute-workers", "open-compute-service"},
    "open-compute-artifacts": {"open-compute-search", "open-compute-images", "open-compute-storage", "open-compute-runtime", "open-compute-workers", "open-compute-service"},
    "open-compute-runtime": {"open-compute-search", "open-compute-images", "open-compute-storage", "open-compute-artifacts", "open-compute-workers", "open-compute-service"},
    "open-compute-workers": {"open-compute-search", "open-compute-images", "open-compute-runtime", "open-compute-service"},
}
errors = []
for name, pkg in by.items():
    banned = forbidden.get(name, set())
    for dep in pkg.get("dependencies", []):
        if dep.get("name") in banned:
            errors.append(f"{name} must not depend on {dep['name']}")
if errors:
    print("\n".join(errors), file=sys.stderr)
    sys.exit(1)
print("dependency boundaries ok")
PY

PYTHONDONTWRITEBYTECODE=1 python3 - <<'PY'
import json
from pathlib import Path
import sys

allowed = {
    "@open-compute/dashboard": {"@open-compute/cloudflare-extension"},
    "@open-compute/cloudflare-extension": set(),
    "@open-compute/runtime": set(),
    "@open-compute/toolchain": set(),
    "@open-compute/workers-types": set(),
}
errors = []
manifest_paths = [
    *Path("apps").glob("*/package.json"),
    *Path("packages").glob("*/package.json"),
]
for manifest_path in manifest_paths:
    manifest = json.loads(manifest_path.read_text())
    name = manifest["name"]
    declared = set()
    for section in ("dependencies", "devDependencies", "peerDependencies", "optionalDependencies"):
        declared.update(key for key in manifest.get(section, {}) if key.startswith("@open-compute/"))
    unexpected = declared - allowed.get(name, set())
    for dependency in sorted(unexpected):
        errors.append(f"{name} must not depend on {dependency}")
if errors:
    print("\n".join(errors), file=sys.stderr)
    sys.exit(1)
print("Bun workspace dependency boundaries ok")
PY
