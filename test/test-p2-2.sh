#!/bin/sh
# Run the stock-workerd Queue producer Gate and the complete P2.1/P1/P0/G0 regression.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
workerd=${OPEN_COMPUTE_TEST_WORKERD:-}
rounds=${OPEN_COMPUTE_GATE_ROUNDS:-3}

case "$rounds" in
  1|2|3) ;;
  *) echo "OPEN_COMPUTE_GATE_ROUNDS must be 1, 2, or 3" >&2; exit 1 ;;
esac
if [ -z "$workerd" ]; then
  case "$(uname -s)-$(uname -m)" in
    Darwin-arm64|Darwin-x86_64|Linux-x86_64|Linux-aarch64|Linux-arm64)
      workerd="$root/poc/.runtime-cache/v1.20260826.1/workerd" ;;
    *) workerd="" ;;
  esac
fi
if [ -z "$workerd" ] || [ ! -f "$workerd" ]; then
  echo "OPEN_COMPUTE_TEST_WORKERD is missing; the P2.2 Gate refuses to skip" >&2
  exit 1
fi
export OPEN_COMPUTE_TEST_WORKERD="$workerd"

cd "$root"
round=1
while [ "$round" -le "$rounds" ]; do
  printf 'P2.2 fresh-process round %s/%s\n' "$round" "$rounds"
  cargo test -p open-compute-service --features test-support \
    --test p2_2_queue_producer_gate -- --test-threads=1 --nocapture
  round=$((round + 1))
done

cargo build -p open-compute-service --bin platformd
if strings "$root/target/debug/platformd" |
  grep -E 'OPEN_COMPUTE_P2_2_FAULT|matrix-json-body|QG-0[1-9]|QG-10' >/dev/null; then
  echo "production platformd contains a P2.2 Gate-only marker or payload" >&2
  exit 1
fi

if [ "$rounds" -eq 3 ]; then
  ./test/test-p2-1.sh
fi
printf 'P2.2 aggregate PASS (%s round(s))\n' "$rounds"
