#!/bin/sh
# Workspace crate dependency boundary check from `cargo metadata`.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
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
    "open-compute-core": {"open-compute-storage", "open-compute-artifacts", "open-compute-runtime", "open-compute-workers", "open-compute-service"},
    "open-compute-storage": {"open-compute-artifacts", "open-compute-runtime", "open-compute-workers", "open-compute-service"},
    "open-compute-artifacts": {"open-compute-storage", "open-compute-runtime", "open-compute-workers", "open-compute-service"},
    "open-compute-runtime": {"open-compute-storage", "open-compute-artifacts", "open-compute-workers", "open-compute-service"},
    "open-compute-workers": {"open-compute-runtime", "open-compute-service"},
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
