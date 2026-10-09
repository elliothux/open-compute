#!/usr/bin/env bash
# Smoke: /stack/* routes with schema, ok:true, and secret-leak checks.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
# shellcheck disable=SC1091
. "${root}/test/stress/common.sh"

run_id=$(date -u +%Y%m%dT%H%M%SZ)-$$
STRESS_RUN_DIR="${stress_run_root}/${run_id}"
export STRESS_RUN_DIR STRESS_BASE_URL="$base_url"
mkdir -p "$STRESS_RUN_DIR"
# shellcheck source=lib/anomaly-check.sh
. "${root}/test/stress/lib/anomaly-check.sh"

capture_container_restart_baseline

curl_stack() {
  local method=$1
  local path=$2
  local body=${3:-}
  local allowed=${4:-200}
  local ctx
  ctx=$(jq -nc --arg method "$method" --arg path "$path" '{method: $method, path: $path}')
  local status body_file
  body_file="${STRESS_RUN_DIR}/last-body.json"
  if [ -n "$body" ]; then
    status=$(curl -sS -H "$host_header" -H 'Content-Type: application/json' \
      -X "$method" -o "$body_file" -w '%{http_code}' \
      --max-time 30 "${base_url}${path}" -d "$body" || echo 000)
  else
    status=$(curl -sS -H "$host_header" -X "$method" -o "$body_file" -w '%{http_code}' \
      --max-time 30 "${base_url}${path}" || echo 000)
  fi
  local response
  response=$(cat "$body_file")
  assert_status_allowed "$status" "$allowed" "$ctx" || true
  assert_json_true "$response" '.ok' "$ctx" || true
  grep_response_secrets "$response" "$ctx" || true
  printf '%s' "$response"
}

check_route() {
  local name=$1
  shift
  printf 'checking %s\n' "$name" >&2
  "$@"
}

check_route "api/health" curl_stack GET "/api/health"
check_route "stack/http/ping" curl_stack GET "/stack/http/ping"

kv_key="smoke-kv-$(date +%s)-$$"
check_route "stack/kv write" curl_stack PUT "/stack/kv/${kv_key}" "smoke-value"
check_route "stack/kv read" curl_stack GET "/stack/kv/${kv_key}"

d1_id="smoke-d1-$(date +%s)-$$"
check_route "stack/d1 write" curl_stack POST "/stack/d1/orders" "{\"orderId\":\"${d1_id}\",\"status\":\"stored\"}" "201"
check_route "stack/d1 read" curl_stack GET "/stack/d1/orders?status=stored&limit=50"

r2_key="smoke-r2-$(date +%s)-$$"
check_route "stack/r2 write" curl_stack PUT "/stack/r2/objects/${r2_key}" "smoke-r2-body"
check_route "stack/r2 read" curl_stack GET "/stack/r2/objects/${r2_key}"

do_id="smoke-do-$(date +%s)-$$"
check_route "stack/do read" curl_stack GET "/stack/do/${do_id}/increment"
check_route "stack/do increment" curl_stack POST "/stack/do/${do_id}/increment" '{"amount":1}'

queue_label="smoke-queue-$(date +%s)-$$"
check_route "stack/queue enqueue" curl_stack POST "/stack/queue/enqueue" "{\"label\":\"${queue_label}\"}"

order_id="smoke-order-$(date +%s)-$$"
check_route "stack/scenario/mega-checkout" curl_stack POST "/stack/scenario/mega-checkout" "{\"orderId\":\"${order_id}\"}"
if ! wait_bindings_aligned "$order_id" 40 >/dev/null; then
  record_anomaly "queue_not_processed" "$(jq -nc --arg order_id "$order_id" '{order_id: $order_id}')"
fi
check_route "stack/scenario/verify" curl_stack GET "/stack/scenario/verify?order_id=${order_id}"

check_health_post_run "$base_url" || true
check_container_restarts || true

python3 - <<PY >"${STRESS_RUN_DIR}/result.json"
import json
print(json.dumps({
  "schema_version": 2,
  "profile": "smoke",
  "run_id": "${run_id}",
  "worker_host": "${worker_host}",
  "stacks": {},
  "scenario": {},
  "global_anomalies": [],
  "verdict": "pass"
}, indent=2))
PY

finalize_verdict "${STRESS_RUN_DIR}/result.json"
