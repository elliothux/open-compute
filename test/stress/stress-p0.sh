#!/usr/bin/env bash
# Per-stack P0 stress for 2C/4G compose profile with per-stack result.json breakdown.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
# shellcheck disable=SC1091
. "${root}/test/stress/common.sh"
ensure_stress_data

run_id=$(date -u +%Y%m%dT%H%M%SZ)-$$
STRESS_RUN_DIR="${stress_run_root}/${run_id}"
export STRESS_RUN_DIR STRESS_BASE_URL="$base_url"
mkdir -p "$STRESS_RUN_DIR"
# shellcheck source=lib/anomaly-check.sh
. "${root}/test/stress/lib/anomaly-check.sh"

capture_container_restart_baseline

STACK_NAMES="http kv d1 r2 queue do workflow fetch cpu service scenario_mega"
for stack in $STACK_NAMES; do
  : >"${STRESS_RUN_DIR}/lat-${stack}.txt"
  : >"${STRESS_RUN_DIR}/err-${stack}.txt"
done

run_stack_load() {
  stack=$1
  concurrency=$2
  path=$3
  method=${4:-GET}
  body=${5:-}
  i=1
  while [ "$i" -le "$concurrency" ]; do
    stress_request "$path" "$method" "$body" \
      "${STRESS_RUN_DIR}/lat-${stack}.txt" \
      "${STRESS_RUN_DIR}/err-${stack}.txt" &
    i=$((i + 1))
  done
  wait
  printf 'completed %s concurrency=%s\n' "$stack" "$concurrency"
}

for concurrency in 10 25 50; do
  run_stack_load http "$concurrency" "/stack/http/ping"
done

kv_key="p0-kv-${run_id}"
run_stack_load kv 20 "/stack/kv/${kv_key}" PUT "p0-value-${run_id}"
run_stack_load kv 20 "/stack/kv/${kv_key}" GET

d1_body='{"status":"created","payloadBytes":128}'
run_stack_load d1 12 "/stack/d1/orders" POST "$d1_body"

i=1
while [ "$i" -le 10 ]; do
  run_stack_load r2 1 "/stack/r2/objects/p0-r2-${run_id}-${i}" PUT "p0-r2-payload-${i}" &
  i=$((i + 1))
done
wait

queue_body='{"batch":[{"label":"p0-q1","payload":{"run":"'"${run_id}"'"}},{"label":"p0-q2","payload":{"run":"'"${run_id}"'"}}]}'
run_stack_load queue 8 "/stack/queue/enqueue" POST "$queue_body"

do_id="p0-do-${run_id}"
run_stack_load do 10 "/stack/do/${do_id}/increment" POST '{"amount":1}'

workflow_body='{"mode":"normal","fanOutN":2}'
run_stack_load workflow 6 "/stack/workflow/checkout" POST "$workflow_body"

run_stack_load fetch 15 "/stack/fetch/probe?hops=1"
run_stack_load cpu 4 "/stack/cpu/spin" POST '{"iterations":40000}'
run_stack_load service 10 "/stack/service/call?mode=rpc"

mega_body='{"mode":"normal","fanOutN":5,"fanOutM":2,"payloadBytes":2048}'
run_stack_load scenario_mega 6 "/stack/scenario/mega-checkout" POST "$mega_body"

check_health_post_run "$base_url" || true
check_container_restarts || true

timestamp=$(date -u +%Y-%m-%dT%H:%M:%SZ)
python3 "${root}/test/stress/report.py" --directory "$STRESS_RUN_DIR" \
  --mode p0 --profile 2c4g --stacks $STACK_NAMES \
  --run-id "$run_id" --timestamp "$timestamp" >"${STRESS_RUN_DIR}/result.json"

finalize_verdict "${STRESS_RUN_DIR}/result.json"
