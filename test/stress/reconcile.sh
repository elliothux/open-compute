#!/usr/bin/env bash
# Cross-binding reconciliation for mega-checkout responses.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
# shellcheck disable=SC1091
. "${root}/test/stress/common.sh"

responses_file=${1:-}
order_id=${STRESS_ORDER_ID:-}

run_id=$(date -u +%Y%m%dT%H%M%SZ)-$$
STRESS_RUN_DIR="${stress_run_root}/${run_id}"
export STRESS_RUN_DIR STRESS_BASE_URL="$base_url"
mkdir -p "$STRESS_RUN_DIR"
# shellcheck source=lib/anomaly-check.sh
. "${root}/test/stress/lib/anomaly-check.sh"

# Queue fan-out and workflow side-effects can lag after peak load or container restart.
STRESS_BINDINGS_ALIGN_SEC=${STRESS_BINDINGS_ALIGN_SEC:-60}

seed_checkout() {
  local oid=$1
  local payload
  payload=$(jq -nc --arg orderId "$oid" '{orderId: $orderId, mode: "normal"}')
  local status body
  status=$(curl -sS -H "$host_header" -H 'Content-Type: application/json' \
    -X POST -o "${STRESS_RUN_DIR}/checkout.json" -w '%{http_code}' \
    --max-time 60 "${base_url}/stack/scenario/mega-checkout" -d "$payload" || echo 000)
  body=$(cat "${STRESS_RUN_DIR}/checkout.json")
  ctx=$(jq -nc --arg order_id "$oid" '{phase: "seed_checkout", order_id: $order_id}')
  assert_status_allowed "$status" "200" "$ctx" || true
  assert_json_true "$body" '.ok' "$ctx" || true
  printf '%s' "$body"
}

reconcile_one() {
  local line=$1
  local idx=$2
  local order_id idem_key queue_label object_id receipt_key
  order_id=$(printf '%s' "$line" | jq -r '.orderId')
  idem_key=$(printf '%s' "$line" | jq -r '.idempotencyKey // empty')
  queue_label=$(printf '%s' "$line" | jq -r '.queue.labels[0] // empty')
  object_id=$(printf '%s' "$line" | jq -r '.inventory.objectId // empty')
  receipt_key=$(printf '%s' "$line" | jq -r '.r2.key // empty')

  if ! wait_bindings_aligned "$order_id" "$STRESS_BINDINGS_ALIGN_SEC"; then
    record_anomaly "reconcile_bindings" "$(jq -nc --arg order_id "$order_id" \
      '{order_id: $order_id, reason: "bindings_not_aligned_before_checks"}')"
  fi

  kv_body=$(curl -sS -H "$host_header" "${base_url}/stack/kv/order:${order_id}" || echo '{}')
  kv_exists=$(printf '%s' "$kv_body" | jq -r '.exists // false')
  if [ "$kv_exists" != "true" ]; then
    record_anomaly "reconcile_kv" "$(jq -nc --arg order_id "$order_id" --argjson body "$(printf '%s' "$kv_body" | jq -c .)" \
      '{order_id: $order_id, kv_exists: false, body: $body}')"
  fi

  d1_body=$(curl -sS -H "$host_header" "${base_url}/stack/d1/orders?status=committed&limit=50" || echo '{}')
  if ! printf '%s' "$d1_body" | jq -e --arg id "$order_id" '.rows[]? | select(.id == $id)' >/dev/null 2>&1; then
    record_anomaly "reconcile_d1" "$(jq -nc --arg order_id "$order_id" \
      '{order_id: $order_id, expected: "committed row in d1"}')"
  fi

  if [ -n "$receipt_key" ]; then
    r2_body=$(curl -sS -H "$host_header" "${base_url}/stack/r2/objects/${receipt_key}" || echo '{}')
    r2_ok=$(printf '%s' "$r2_body" | jq -r '.ok // false')
    if [ "$r2_ok" != "true" ]; then
      record_anomaly "reconcile_r2" "$(jq -nc --arg order_id "$order_id" --arg key "$receipt_key" \
        '{order_id: $order_id, key: $key, r2_ok: false}')"
    fi
  fi

  if [ -n "$object_id" ]; then
    do_body=$(curl -sS -H "$host_header" -X POST "${base_url}/stack/do/${object_id}/increment" \
      -H 'Content-Type: application/json' -d '{"amount":0}' || echo '{}')
    do_count=$(printf '%s' "$do_body" | jq -r '.count // 0')
    if [ "$do_count" -lt 1 ]; then
      record_anomaly "reconcile_do" "$(jq -nc --arg order_id "$order_id" --arg object_id "$object_id" \
        --argjson count "$do_count" '{order_id: $order_id, object_id: $object_id, count: $count}')"
    fi
  fi

  if [ -n "$queue_label" ]; then
    if ! wait_queue_label "$queue_label" 30; then
      record_anomaly "reconcile_queue" "$(jq -nc --arg order_id "$order_id" --arg label "$queue_label" \
        '{order_id: $order_id, label: $label, processed: false}')"
    fi
  fi

  verify_body=$(curl -sS -H "$host_header" \
    "${base_url}/stack/scenario/verify?order_id=${order_id}" || echo '{}')
  consistent=$(printf '%s' "$verify_body" | jq -r '.consistent // false')
  bindings=$(printf '%s' "$verify_body" | jq -r '.bindings_aligned // false')
  if [ "$consistent" != "true" ]; then
    record_anomaly "reconcile_consistency" "$(jq -nc --arg order_id "$order_id" \
      --argjson verify "$(printf '%s' "$verify_body" | jq -c .)" '{order_id: $order_id, verify: $verify}')"
  fi
  if [ "$bindings" != "true" ]; then
    record_anomaly "reconcile_bindings" "$(jq -nc --arg order_id "$order_id" \
      --argjson verify "$(printf '%s' "$verify_body" | jq -c .)" '{order_id: $order_id, verify: $verify}')"
  fi

  if [ -n "$idem_key" ]; then
    idem_body=$(curl -sS -H "$host_header" "${base_url}/stack/kv/${idem_key}" || echo '{}')
    idem_exists=$(printf '%s' "$idem_body" | jq -r '.exists // false')
    if [ "$idem_exists" != "true" ]; then
      record_anomaly "reconcile_idempotency" "$(jq -nc --arg order_id "$order_id" --arg key "$idem_key" \
        '{order_id: $order_id, idempotency_key: $key, exists: false}')"
    fi
  fi

  printf 'reconciled order %s (index %s)\n' "$order_id" "$idx"
}

if [ -n "$responses_file" ] && [ -f "$responses_file" ]; then
  idx=0
  while IFS= read -r line; do
    [ -z "$line" ] && continue
    reconcile_one "$line" "$idx"
    idx=$((idx + 1))
  done <<EOF
$(grep -v '^[[:space:]]*$' "$responses_file")
EOF
  reconciled=$idx
else
  order_id=${order_id:-"reconcile-${run_id}"}
  line=$(seed_checkout "$order_id")
  reconcile_one "$line" 0
  reconciled=1
fi

check_health_post_run "$base_url" || true

python3 - <<PY >"${STRESS_RUN_DIR}/result.json"
import json
print(json.dumps({
  "schema_version": 2,
  "profile": "reconcile",
  "run_id": "${run_id}",
  "reconciled": ${reconciled:-0},
  "worker_host": "${worker_host}",
  "stacks": {
    "kv": {"samples": ${reconciled:-0}, "latency_ms": {"p50": 0, "p95": 0, "p99": 0}, "anomalies": [], "verdict": "pass"},
    "d1": {"samples": ${reconciled:-0}, "latency_ms": {"p50": 0, "p95": 0, "p99": 0}, "anomalies": [], "verdict": "pass"},
    "r2": {"samples": ${reconciled:-0}, "latency_ms": {"p50": 0, "p95": 0, "p99": 0}, "anomalies": [], "verdict": "pass"},
    "do": {"samples": ${reconciled:-0}, "latency_ms": {"p50": 0, "p95": 0, "p99": 0}, "anomalies": [], "verdict": "pass"},
    "queue": {"samples": ${reconciled:-0}, "latency_ms": {"p50": 0, "p95": 0, "p99": 0}, "anomalies": [], "verdict": "pass"}
  },
  "scenario": {
    "mega-checkout": {"anomalies": [], "verdict": "pass"}
  },
  "global_anomalies": [],
  "verdict": "pass"
}, indent=2))
PY

finalize_verdict "${STRESS_RUN_DIR}/result.json"
