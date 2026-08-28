#!/bin/sh
# Run bounded failure/restart cleanup and snapshot corruption checks in fresh processes.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
workerd=${OPEN_COMPUTE_TEST_WORKERD:-"$root/poc/.runtime-cache/v1.20260826.1/workerd"}
if [ ! -f "$workerd" ]; then
  echo "OPEN_COMPUTE_TEST_WORKERD is missing; the P1 crash Gate refuses to skip" >&2
  exit 1
fi
export OPEN_COMPUTE_TEST_WORKERD="$workerd"
cd "$root"
round=1
while [ "$round" -le 3 ]; do
  printf 'P1 crash/recovery round %s/3\n' "$round"
  cargo test -p open-compute-service --all-features \
    run_startup_failure_matrix_releases_owned_resources -- --test-threads=1
  cargo test -p open-compute-artifacts --all-features \
    p1_snapshot_layout_commits_manifest_last_and_verifies_exact_bytes -- --test-threads=1
  cargo test -p open-compute-storage --all-features \
    p1_admission_lock_restore_target_and_forward_upgrade_fail_closed -- --test-threads=1
  cargo test -p open-compute-service --features test-support \
    --test p1_crash_process -- --test-threads=1
  cargo test -p open-compute-service --features test-support \
    --test p0_exit_gate -- --test-threads=1
  round=$((round + 1))
done
