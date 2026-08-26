#!/bin/sh
# Run the deterministic P1.0 capability/deviation contract in fresh platformd processes.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
workerd=${OPEN_COMPUTE_TEST_WORKERD:-"$root/poc/.runtime-cache/v1.20260823.1/workerd"}
if [ ! -f "$workerd" ]; then
  echo "OPEN_COMPUTE_TEST_WORKERD is missing; the P1 conformance Gate refuses to skip" >&2
  exit 1
fi
export OPEN_COMPUTE_TEST_WORKERD="$workerd"
cd "$root"
round=1
while [ "$round" -le 3 ]; do
  printf 'P1.0 conformance fresh-process round %s/3\n' "$round"
  cargo test -p open-compute-service --features test-support \
    --test p1_conformance -- --test-threads=1 --nocapture
  cargo test -p open-compute-service --features test-support \
    --test p0_exit_gate -- --test-threads=1 --nocapture
  round=$((round + 1))
done
