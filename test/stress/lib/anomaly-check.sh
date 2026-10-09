#!/usr/bin/env bash
# Shared anomaly detection helpers for stress scripts.
# shellcheck disable=SC2034
set -eu

: "${STRESS_RUN_DIR:?set STRESS_RUN_DIR before sourcing anomaly-check.sh}"
: "${STRESS_BASE_URL:?set STRESS_BASE_URL}"

ANOMALY_FILE="${STRESS_RUN_DIR}/anomalies.ndjson"
ANOMALY_RESULT="${STRESS_RUN_DIR}/anomalies.json"
CONTAINER_RESTART_BASELINE="${STRESS_RUN_DIR}/.container_restart_baseline"

mkdir -p "$STRESS_RUN_DIR"
touch "$ANOMALY_FILE"

_anomaly_now() {
  date -u +%Y-%m-%dT%H:%M:%SZ
}

record_anomaly() {
  local type=$1
  local details_json=$2
  jq -nc \
    --arg type "$type" \
    --arg ts "$(_anomaly_now)" \
    --argjson details "$details_json" \
    '{type: $type, timestamp: $ts, details: $details}' >>"$ANOMALY_FILE"
}

assert_status_allowed() {
  local status=$1
  local allowed_csv=$2
  local context_json=${3:-'{}'}
  local allowed
  IFS=',' read -r -a allowed <<<"$allowed_csv"
  for code in "${allowed[@]}"; do
    if [ "$status" = "$code" ]; then
      return 0
    fi
  done
  record_anomaly "http_status" "$(jq -nc \
    --arg status "$status" \
    --arg allowed "$allowed_csv" \
    --argjson context "$context_json" \
    '{status: $status, allowed: ($allowed | split(",")), context: $context}')"
  return 1
}

assert_json_field() {
  local body=$1
  local jq_path=$2
  local expected=$3
  local context_json=${4:-'{}'}
  if ! printf '%s' "$body" | jq -e . >/dev/null 2>&1; then
    record_anomaly "json_parse" "$(jq -nc \
      --arg path "$jq_path" \
      --arg expected "$expected" \
      --argjson context "$context_json" \
      '{path: $path, expected: $expected, context: $context, reason: "invalid_json"}')"
    return 1
  fi
  local actual
  actual=$(printf '%s' "$body" | jq -r "$jq_path | if . == null then \"\" else tostring end")
  if [ "$actual" != "$expected" ]; then
    record_anomaly "json_field" "$(jq -nc \
      --arg path "$jq_path" \
      --arg expected "$expected" \
      --arg actual "$actual" \
      --argjson context "$context_json" \
      '{path: $path, expected: $expected, actual: $actual, context: $context}')"
    return 1
  fi
  return 0
}

assert_json_true() {
  local body=$1
  local jq_path=$2
  local context_json=${3:-'{}'}
  assert_json_field "$body" "$jq_path" "true" "$context_json"
}

grep_response_secrets() {
  local body=$1
  local context_json=${2:-'{}'}
  local patterns=(
    'Bearer[[:space:]]+[A-Za-z0-9._-]{16,}'
    '[Aa][Pp][Ii][_-]?[Kk][Ee][Yy][[:space:]*:=]+["'"'"']?[A-Za-z0-9._-]{16,}'
    '[Ss][Ee][Cc][Rr][Ee][Tt][_-]?[Kk][Ee][Yy][[:space:]*:=]+["'"'"']?[A-Za-z0-9._-]{16,}'
    '"token"[[:space:]]*:[[:space:]]*"[A-Za-z0-9._-]{32,}"'
    '"password"[[:space:]]*:[[:space:]]*"[^"]{8,}"'
    'OPEN_COMPUTE_[A-Z_]*=[A-Za-z0-9._-]{16,}'
  )
  local pattern match
  for pattern in "${patterns[@]}"; do
    match=$(printf '%s' "$body" | grep -Eo "$pattern" 2>/dev/null | head -1 || true)
    if [ -n "$match" ]; then
      record_anomaly "secret_leak" "$(jq -nc \
        --arg pattern "$pattern" \
        --arg match "$match" \
        --argjson context "$context_json" \
        '{pattern: $pattern, match: $match, context: $context}')"
      return 1
    fi
  done
  return 0
}

check_health_post_run() {
  local base_url=$1
  local live ready
  live=$(curl -fsS -o /dev/null -w '%{http_code}' "${base_url}/health/live" 2>/dev/null || echo 000)
  ready=$(curl -fsS -o /dev/null -w '%{http_code}' "${base_url}/health/ready" 2>/dev/null || echo 000)
  local ok=0
  if [ "$live" != "200" ]; then
    record_anomaly "health_live" "$(jq -nc --arg status "$live" '{status: $status}')"
    ok=1
  fi
  if [ "$ready" != "200" ]; then
    record_anomaly "health_ready" "$(jq -nc --arg status "$ready" '{status: $status}')"
    ok=1
  fi
  return "$ok"
}

capture_container_restart_baseline() {
  local container_name
  if ! container_name=$(stress_container); then
    record_anomaly "container_lookup" '{}'
    return 1
  fi
  local count
  if ! count=$(docker inspect --format='{{.RestartCount}}' "$container_name"); then
    record_anomaly "container_inspection" '{}'
    return 1
  fi
  printf '%s\n' "$count" >"$CONTAINER_RESTART_BASELINE"
}

check_container_restarts() {
  local container_name
  if ! container_name=$(stress_container); then
    record_anomaly "container_lookup" '{}'
    return 1
  fi
  if [ ! -f "$CONTAINER_RESTART_BASELINE" ]; then
    capture_container_restart_baseline
    return $?
  fi
  local before after delta
  before=$(cat "$CONTAINER_RESTART_BASELINE")
  if ! after=$(docker inspect --format='{{.RestartCount}}' "$container_name"); then
    record_anomaly "container_inspection" '{}'
    return 1
  fi
  delta=$((after - before))
  if [ "$delta" -gt 0 ]; then
    record_anomaly "container_restart" "$(jq -nc \
      --arg container "$container_name" \
      --argjson before "$before" \
      --argjson after "$after" \
      --argjson delta "$delta" \
      '{container: $container, before: $before, after: $after, delta: $delta}')"
    return 1
  fi
  return 0
}

anomaly_count() {
  if [ ! -s "$ANOMALY_FILE" ]; then
    echo 0
    return
  fi
  wc -l <"$ANOMALY_FILE" | tr -d ' '
}

load_anomalies_array() {
  if [ ! -s "$ANOMALY_FILE" ]; then
    echo '[]'
    return
  fi
  jq -s '.' "$ANOMALY_FILE"
}

finalize_verdict() {
  local result_json=${1:-}
  if [ -n "$result_json" ]; then
    if [ ! -f "$result_json" ] || ! jq -e '.verdict == "pass"' "$result_json" >/dev/null 2>&1; then
      record_anomaly "qualification_result" '{"reason":"missing, invalid or failed verdict"}'
    fi
  fi
  local count
  count=$(anomaly_count)
  local anomalies
  anomalies=$(load_anomalies_array)
  local verdict=pass
  if [ "$count" -gt 0 ]; then
    verdict=fail
  fi

  if [ -n "$result_json" ] && [ -f "$result_json" ]; then
    local merged
    merged=$(jq \
      --argjson global_anomalies "$anomalies" \
      --arg verdict "$verdict" \
      '.global_anomalies = $global_anomalies | .verdict = (if $verdict == "fail" then "fail" else .verdict end)' \
      "$result_json")
    if [ "$verdict" = "fail" ]; then
      merged=$(printf '%s' "$merged" | jq '.verdict = "fail"')
    fi
    printf '%s\n' "$merged" >"$result_json"
  fi

  jq -nc \
    --arg verdict "$verdict" \
    --argjson anomalies "$anomalies" \
    --arg run_id "$(basename "$STRESS_RUN_DIR")" \
    --arg timestamp "$(_anomaly_now)" \
    '{run_id: $run_id, timestamp: $timestamp, verdict: $verdict, anomalies: $anomalies}' \
    >"$ANOMALY_RESULT"

  if [ "$verdict" = "fail" ]; then
    local failed_dir="${STRESS_RUN_DIR}/failed"
    mkdir -p "$failed_dir"
    cp "$ANOMALY_RESULT" "${failed_dir}/anomalies.json"
    if [ -n "$result_json" ] && [ -f "$result_json" ]; then
      cp "$result_json" "${failed_dir}/result.json"
    fi
    printf 'anomaly-check: FAIL (%s anomalies) -> %s\n' "$count" "${failed_dir}/anomalies.json" >&2
    exit 1
  fi

  printf 'anomaly-check: PASS (0 anomalies)\n'
}
