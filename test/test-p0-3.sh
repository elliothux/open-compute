#!/bin/sh
# Run the stock-workerd resource-binding matrix in three fresh processes, then P0.2.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
workerd=${OPEN_COMPUTE_TEST_WORKERD:-}

if [ -z "$workerd" ]; then
  case "$(uname -s)-$(uname -m)" in
    Darwin-arm64|Darwin-x86_64|Linux-x86_64|Linux-aarch64|Linux-arm64)
      workerd="$root/poc/.runtime-cache/v1.20260826.1/workerd" ;;
    *) workerd="" ;;
  esac
fi
if [ -z "$workerd" ] || [ ! -f "$workerd" ]; then
  echo "OPEN_COMPUTE_TEST_WORKERD is missing; the P0.3 Gate refuses to skip" >&2
  exit 1
fi

export OPEN_COMPUTE_TEST_WORKERD="$workerd"
cd "$root"
round=1
while [ "$round" -le 3 ]; do
  printf 'P0.3 fresh-process round %s/3\n' "$round"
  cargo test -p open-compute-service --features test-support \
    --test p0_3_resource_binding_gate -- --test-threads=1 --nocapture
  round=$((round + 1))
done

./test/test-p0-2.sh
