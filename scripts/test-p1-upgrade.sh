#!/bin/sh
# Run the P1 forward-only schema resume and release-package contract Gates.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
workerd=${OPEN_COMPUTE_TEST_WORKERD:-"$root/poc/.runtime-cache/v1.20260826.1/workerd"}
if [ ! -f "$workerd" ]; then
  echo "OPEN_COMPUTE_TEST_WORKERD is missing; the P1 upgrade Gate refuses to skip" >&2
  exit 1
fi
export OPEN_COMPUTE_TEST_WORKERD="$workerd"
cd "$root"
round=1
while [ "$round" -le 3 ]; do
  printf 'P1 upgrade/rollback-anchor round %s/3\n' "$round"
  cargo test -p open-compute-service --features test-support \
    --test p1_upgrade -- --test-threads=1
  cargo test -p open-compute-storage --all-features \
    p1_admission_lock_restore_target_and_forward_upgrade_fail_closed -- --test-threads=1
  cargo test -p open-compute-runtime --all-features \
    package_release_is_atomic_and_rejects_bad_inputs -- --test-threads=1
  round=$((round + 1))
done
