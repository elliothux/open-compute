#!/bin/sh
# Run the stock-workerd Queue consumer/Cron Gate and durable authority regressions.
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
  echo "OPEN_COMPUTE_TEST_WORKERD is missing; the P2.3 Gate refuses to skip" >&2
  exit 1
fi
export OPEN_COMPUTE_TEST_WORKERD="$workerd"

cd "$root"
round=1
while [ "$round" -le "$rounds" ]; do
  printf 'P2.3 stock-workerd fresh-process round %s/%s\n' "$round" "$rounds"
  cargo test -p open-compute-service --test p0_2_runtime_gate \
    -- --test-threads=1 --nocapture
  round=$((round + 1))
done

cargo test -p open-compute-core cron::tests -- --test-threads=1
cargo test -p open-compute-storage queue_consumer_claim_completion_recovery_and_dlq_are_token_fenced \
  -- --test-threads=1
cargo test -p open-compute-storage cron_slots_retries_and_unknown_recovery_preserve_logical_identity \
  -- --test-threads=1
cargo test -p open-compute-storage migration_003_upgrades_a_real_v2_backlog_and_preserves_producer_delivery \
  -- --test-threads=1
cargo test -p open-compute-storage migration_004_preserves_a_real_v3_claim_without_mutating_queue_authority \
  -- --test-threads=1
cargo test -p open-compute-service --lib \
  p2_3_promotion_is_idempotent_preserves_pause_and_resumes_an_interrupted_update \
  -- --test-threads=1

cargo build -p open-compute-service --bin platformd
if strings "$root/target/debug/platformd" |
  grep -E 'runtime-gate-(throw|wait-until|timeout)|p23-(first|second|third)' >/dev/null; then
  echo "production platformd contains a P2.3 Gate-only marker" >&2
  exit 1
fi

printf 'P2.3 Queue consumer/Cron Gate PASS (%s fresh-process round(s))\n' "$rounds"
