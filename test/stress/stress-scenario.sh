#!/usr/bin/env bash
# Scenario stress: mega-checkout success + fault-mode structured errors.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
# shellcheck disable=SC1091
. "${root}/test/stress/common.sh"

mode=${STRESS_SCENARIO_MODE:-normal}

run_id=$(date -u +%Y%m%dT%H%M%SZ)-$$
STRESS_RUN_DIR="${stress_run_root}/${run_id}"
export STRESS_RUN_DIR STRESS_BASE_URL="$base_url"
mkdir -p "$STRESS_RUN_DIR"
# shellcheck source=lib/anomaly-check.sh
. "${root}/test/stress/lib/anomaly-check.sh"

capture_container_restart_baseline

post_checkout() {
  local order_id=$1 payload=$2 expected_status=${3:-200}
  local ctx body_file status body
  ctx=$(jq -nc --arg order_id "$order_id" --arg mode "$mode" '{order_id: $order_id, mode: $mode}')
  body_file="${STRESS_RUN_DIR}/checkout-${order_id}.json"
  status=$(curl -sS -H "$host_header" -H 'Content-Type: application/json' \
    -X POST -o "$body_file" -w '%{http_code}' \
    --max-time 60 "${base_url}/stack/scenario/mega-checkout" -d "$payload" || echo 000)
  body=$(cat "$body_file")

  if [ "$mode" = "fault" ]; then
    assert_status_allowed "$status" "503" "$ctx" || true
    assert_json_field "$body" '.ok' "false" "$ctx" || true
    error_code=$(printf '%s' "$body" | jq -r '.error_code // empty')
    stack=$(printf '%s' "$body" | jq -r '.stack // empty')
    if [ -z "$error_code" ] || [ -z "$stack" ]; then
      record_anomaly "fault_missing_fields" "$(jq -nc --arg body "$body" '{body: $body}')"
    fi
    verify=$(curl -sS -H "$host_header" \
      "${base_url}/stack/scenario/verify?order_id=${order_id}" 2>/dev/null || echo '{}')
    consistent=$(printf '%s' "$verify" | jq -r '.consistent // empty')
    if [ "$consistent" = "true" ]; then
      record_anomaly "fault_state_corruption" "$(jq -nc \
        --arg order_id "$order_id" \
        --argjson verify "$(printf '%s' "$verify" | jq -c . 2>/dev/null || echo '{}')" \
        '{order_id: $order_id, verify: $verify}')"
    fi
    grep_response_secrets "$body" "$ctx" || true
  else
    assert_status_allowed "$status" "$expected_status" "$ctx" || true
    assert_json_true "$body" '.ok' "$ctx" || true
    assert_json_field "$body" '.status' "committed" "$ctx" || true
    grep_response_secrets "$body" "$ctx" || true
  fi
  printf '%s' "$body"
}

order_id="scenario-${mode}-${run_id}"
if [ "$mode" = "fault" ]; then
  fault_stack=${STRESS_FAULT_STACK:-kv}
  payload=$(jq -nc \
    --arg orderId "$order_id" \
    --arg faultStack "$fault_stack" \
    '{orderId: $orderId, mode: "fault", faultStack: $faultStack}')
  post_checkout "$order_id" "$payload" 503 >/dev/null
else
  payload=$(jq -nc --arg orderId "$order_id" '{orderId: $orderId, mode: "normal"}')
  post_checkout "$order_id" "$payload" 200 >/dev/null

  if ! verify_body=$(wait_bindings_aligned "$order_id" 40); then
    verify_body=$(curl -sS -H "$host_header" \
      "${base_url}/stack/scenario/verify?order_id=${order_id}" || echo '{}')
    record_anomaly "queue_not_processed" "$(jq -nc --arg order_id "$order_id" '{order_id: $order_id}')"
  fi
  ctx=$(jq -nc --arg order_id "$order_id" '{order_id: $order_id, phase: "verify"}')
  assert_json_true "$verify_body" '.ok' "$ctx" || true
  assert_json_true "$verify_body" '.consistent' "$ctx" || true
  assert_json_true "$verify_body" '.bindings_aligned' "$ctx" || true
fi

check_health_post_run "$base_url" || true
check_container_restarts || true

python3 - <<PY >"${STRESS_RUN_DIR}/result.json"
import json
print(json.dumps({
  "schema_version": 2,
  "profile": "scenario",
  "run_id": "${run_id}",
  "mode": "${mode}",
  "worker_host": "${worker_host}",
  "stacks": {},
  "scenario": {
    "mega-checkout": {"anomalies": [], "verdict": "pass"}
  },
  "global_anomalies": [],
  "verdict": "pass"
}, indent=2))
PY

finalize_verdict "${STRESS_RUN_DIR}/result.json"
