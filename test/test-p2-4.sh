#!/bin/sh
# Run the sequential Workflow engine through verified stock workerd and both real authorities.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
workerd=${OPEN_COMPUTE_TEST_WORKERD:-"$root/.temp/runtime-cache/v1.20260826.1/workerd"}
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
  echo "OPEN_COMPUTE_TEST_WORKERD is missing; the Workflow Gate refuses to skip" >&2
  exit 1
fi
export OPEN_COMPUTE_TEST_WORKERD="$workerd"
cd "$root"
round=1
while [ "$round" -le "$rounds" ]; do
  printf 'P2.4 stock-workerd fresh-process round %s/%s\n' "$round" "$rounds"
  cargo test -p open-compute-service --all-features \
    --test p2_4_workflow_hard_gate --test p2_4_workflow_product_gate \
    -- --test-threads=1
  round=$((round + 1))
done
cargo test -p open-compute-core -p open-compute-storage -p open-compute-workers \
  --lib workflow --all-features -- --test-threads=1
cargo test -p open-compute-service --lib workflow --all-features -- --test-threads=1
printf 'P2.4 Workflow Gate PASS (%s fresh-process round(s))\n' "$rounds"
