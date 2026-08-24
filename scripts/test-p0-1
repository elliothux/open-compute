#!/bin/sh
# Locate/verify the pinned workerd, then run the three-round process Gate.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
lock="$root/runtime/workerd.lock.json"
workerd=${OPEN_COMPUTE_TEST_WORKERD:-}

if [ -z "$workerd" ]; then
  case "$(uname -s)-$(uname -m)" in
    Darwin-arm64|Darwin-x86_64|Linux-x86_64|Linux-aarch64|Linux-arm64)
      rel="v1.20260823.1"; cache="$root/poc/.runtime-cache/$rel/workerd" ;;
    *) cache="" ;;
  esac
  if [ -n "$cache" ] && [ -f "$cache" ]; then
    workerd=$cache
  fi
fi

if [ -z "$workerd" ] || [ ! -f "$workerd" ]; then
  echo "OPEN_COMPUTE_TEST_WORKERD is missing; the Gate refuses to skip" >&2
  exit 1
fi

python3 - "$lock" "$workerd" <<'PY'
import hashlib, json, os, sys
lock_path, binary = sys.argv[1], sys.argv[2]
lock = json.load(open(lock_path))
os_name = os.uname().sysname.lower()
machine = os.uname().machine
if os_name == "darwin" and machine == "arm64":
    target = "darwin-arm64"
elif os_name == "darwin" and machine == "x86_64":
    target = "darwin-x64"
elif os_name == "linux" and machine in ("x86_64", "amd64"):
    target = "linux-x64"
elif os_name == "linux" and machine in ("aarch64", "arm64"):
    target = "linux-arm64"
else:
    print(f"unsupported host {os_name}-{machine}", file=sys.stderr)
    sys.exit(1)
info = lock.get("targets", {}).get(target)
if info is None:
    print(f"workerd.lock.json has no target {target}; refusing to fetch latest", file=sys.stderr)
    sys.exit(1)
digest = hashlib.sha256(open(binary, "rb").read()).hexdigest()
if digest != info["binarySha256"]:
    print("workerd hash does not match the formal lock", file=sys.stderr)
    sys.exit(1)
print(f"verified {target} {lock['expectedVersionOutput']} {digest}")
PY

export OPEN_COMPUTE_TEST_WORKERD="$workerd"
cd "$root"
exec cargo test -p open-compute-service --test p0_1_gate -- --test-threads=1 --nocapture
