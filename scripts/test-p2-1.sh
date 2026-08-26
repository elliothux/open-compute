#!/bin/sh
# Run P2.1 fresh-process crash/scope Gates and the complete P1/P0/G0 regression.
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
  echo "OPEN_COMPUTE_TEST_WORKERD is missing; the P2.1 Gate refuses to skip" >&2
  exit 1
fi
export OPEN_COMPUTE_TEST_WORKERD="$workerd"

cd "$root"
round=1
while [ "$round" -le 3 ]; do
  printf 'P2.1 fresh-process round %s/3\n' "$round"
  cargo test -p open-compute-service --features test-support \
    --test p2_1_scheduler_hardening_gate -- --test-threads=1 --nocapture
  round=$((round + 1))
done

cargo build -p open-compute-service --bin platformd
if strings "$root/target/debug/platformd" |
  grep -E 'AfterClaimCommit|BeforeDispatch|AfterDispatchBeforeComplete|AfterCompleteCommit|DuringProjectionRefresh' >/dev/null; then
  echo "production platformd contains a scheduler fault-point marker" >&2
  exit 1
fi

./scripts/test-p1.sh
./poc/g0 test all
printf 'P2.1 aggregate PASS\n'
