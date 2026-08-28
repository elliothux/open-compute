#!/bin/sh
# Full HTTP/Queue/Workflow/product chain, with actual platformd crash and recovery.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
workerd=${OPEN_COMPUTE_TEST_WORKERD:-"$root/poc/.runtime-cache/v1.20260826.1/workerd"}
rounds=${OPEN_COMPUTE_GATE_ROUNDS:-3}
case "$rounds" in
  1|2|3) ;;
  *) echo "OPEN_COMPUTE_GATE_ROUNDS must be 1, 2, or 3" >&2; exit 1 ;;
esac
case "$workerd" in
  /*) ;;
  *) echo "OPEN_COMPUTE_TEST_WORKERD must be absolute" >&2; exit 1 ;;
esac
if [ ! -f "$workerd" ]; then
  echo "OPEN_COMPUTE_TEST_WORKERD is missing; the P2 Exit Gate refuses to skip" >&2
  exit 1
fi
export OPEN_COMPUTE_TEST_WORKERD="$workerd"
cd "$root"
round=1
while [ "$round" -le "$rounds" ]; do
  printf 'P2 Exit stock-workerd fresh-process round %s/%s\n' "$round" "$rounds"
  cargo test -p open-compute-service --all-features --test p2_exit_gate -- --test-threads=1
  round=$((round + 1))
done
# The chain cuts complement, rather than replace, the authority transaction matrices.
workflow_crash_test='workflows::tests::crash_matrix::durable::workflow_sigkill_durable_wait_retry_pause_restart_and_purge_boundaries'
cargo test -p open-compute-workers --lib --all-features "$workflow_crash_test" -- --exact --list |
  grep -Fxq "$workflow_crash_test: test"
cargo test -p open-compute-workers --lib --all-features "$workflow_crash_test" -- --exact --test-threads=1
cargo test -p open-compute-service --all-features --test p2_2_queue_producer_gate commit_crash -- --test-threads=1
printf 'P2 Exit Gate PASS (%s fresh-process round(s))\n' "$rounds"
