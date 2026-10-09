#!/bin/sh
# Shared helpers for curl-based stress harnesses.
set -eu

_resolve_open_compute_root() {
  if [ -n "${OPEN_COMPUTE_ROOT:-}" ] && [ -f "${OPEN_COMPUTE_ROOT}/Cargo.toml" ]; then
    printf '%s' "$OPEN_COMPUTE_ROOT"
    return 0
  fi
  case "$0" in
    */test/stress/*)
      CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd
      return 0
      ;;
  esac
  if [ -f "./Cargo.toml" ] && [ -d "./test/stress" ]; then
    pwd
    return 0
  fi
  echo "cannot resolve open-compute repository root (set OPEN_COMPUTE_ROOT or run from repo root)" >&2
  return 1
}
root=$(_resolve_open_compute_root)
stress_data_dir="${root}/.temp/stress-data"
stress_run_root="${STRESS_RUN_ROOT:-${root}/.temp/stress-run}"

base_url=${STRESS_BASE_URL:-http://127.0.0.1:8787}
if [ -z "${STRESS_ACCOUNT_ID:-}" ] && [ -f "${stress_run_root}/.deploy_env" ]; then
  set -a
  # shellcheck disable=SC1091
  . "${stress_run_root}/.deploy_env"
  set +a
fi
account_id=${STRESS_ACCOUNT_ID:?set STRESS_ACCOUNT_ID}
worker_host=${STRESS_WORKER_HOST:-stress-demo.${account_id}.localhost}
host_header="Host: ${worker_host}"

ensure_stress_data() {
  if [ ! -f "${stress_data_dir}/manifest.json" ]; then
    sh "${root}/test/stress/generate-data.sh"
  fi
}

stress_compose() {
  if [ -n "${STRESS_COMPOSE_OVERRIDE:-}" ]; then
    docker compose --project-directory "${root}/examples/container" \
      -f "${root}/examples/container/docker-compose.yml" -f "$STRESS_COMPOSE_OVERRIDE" "$@"
    return
  fi
  docker compose --project-directory "${root}/examples/container" \
    -f "${root}/examples/container/docker-compose.yml" "$@"
}

stress_container() {
  container=$(stress_compose ps -q ocd) || return 1
  case "$container" in
    ""|*'
'*) echo "expected one running ocd in the selected Compose project" >&2; return 1 ;;
  esac
  printf '%s\n' "$container"
}

restart_stress_container() {
  container=$(stress_container) || return 1
  before=$(docker inspect --format '{{.State.StartedAt}}' "$container") || return 1
  stress_compose restart ocd || return 1
  after=$(docker inspect --format '{{.State.StartedAt}}' "$container") || return 1
  if [ "$before" = "$after" ]; then
    echo "ocd container did not restart" >&2
    return 1
  fi
}

recover_before_sample() (
  settle_sec=${1:-${STRESS_RECOVER_SETTLE_SEC:-10}}
  wait_for_worker_ready 90 || return 1
  sleep "$settle_sec"
)

require_disk_space() {
  min_gb=${1:-5}
  check_path=${2:-${root}/.temp}
  mkdir -p "$check_path"
  avail_kb=$(df -k "$check_path" | awk 'NR==2 {print $4}')
  min_kb=$((min_gb * 1024 * 1024))
  if [ "$avail_kb" -lt "$min_kb" ]; then
    avail_gb=$(python3 - <<PY
print(f"{${avail_kb} / (1024 * 1024):.2f}")
PY
)
    echo "insufficient disk space: need ${min_gb}GB free in ${check_path}, found ${avail_gb}GB" >&2
    exit 1
  fi
}

now_ms() {
  python3 -c 'import time; print(int(time.time() * 1000))'
}

percentile() {
  pct=$1
  file=$2
  count=$(wc -l <"$file" | tr -d ' ')
  if [ "$count" -eq 0 ]; then
    echo 0
    return
  fi
  rank=$(((count * pct + 99) / 100))
  sort -n "$file" | awk -v rank="$rank" 'NR == rank { print; exit }'
}

stack_stats() {
  lat_file=$1
  err_file=$2
  total=$(wc -l <"$lat_file" | tr -d ' ')
  errors=$(wc -l <"$err_file" | tr -d ' ')
  if [ "$total" -eq 0 ]; then
    error_rate=1
  else
    error_rate=$(python3 - <<PY
total = ${total}
errors = ${errors}
print(f"{errors / total:.6f}")
PY
)
  fi
  p50=$(percentile 50 "$lat_file")
  p95=$(percentile 95 "$lat_file")
  p99=$(percentile 99 "$lat_file")
  python3 - <<PY
import json
print(json.dumps({
  "samples": ${total},
  "errors": ${errors},
  "error_rate": float("${error_rate}"),
  "p50": ${p50},
  "p95": ${p95},
  "p99": ${p99},
}))
PY
}

stress_request() {
  path=$1
  method=${2:-GET}
  body=${3:-}
  lat_file=$4
  err_file=$5
  request_failed=0
  status=000
  body_out=/dev/null
  if [ -n "$body" ]; then
    if ! response=$(curl -sS -H "$host_header" -H 'Content-Type: application/json' \
      -X "$method" -o "$body_out" -w '%{http_code} %{time_total}' \
      --max-time 60 "${base_url}${path}" -d "$body"); then
      request_failed=1
    fi
  else
    if ! response=$(curl -sS -H "$host_header" -X "$method" -o "$body_out" -w '%{http_code} %{time_total}' \
      --max-time 60 "${base_url}${path}"); then
      request_failed=1
    fi
  fi
  IFS=' ' read -r status elapsed_secs <<EOF
$response
EOF
  [ "$request_failed" -eq 0 ] || status=000
  latency=$(awk -v seconds="$elapsed_secs" 'BEGIN {
    if (seconds !~ /^[0-9]+([.][0-9]+)?$/) exit 1
    printf "%.0f\n", seconds * 1000
  }')
  printf '%s\n' "$latency" >>"$lat_file"
  case "$status" in
    200|201|202|204|101) ;;
    *) printf '%s %s %s\n' "$method" "$path" "$status" >>"$err_file" ;;
  esac
}

# Idempotent GET with bounded retry for transient 5xx under peak load.
stress_request_idempotent_get() {
  path=$1
  lat_file=$2
  err_file=$3
  max_attempts=${4:-5}
  start_ms=$(now_ms)
  status=000
  body_out=/dev/null
  attempt=1
  while [ "$attempt" -le "$max_attempts" ]; do
    if ! response=$(curl -sS -H "$host_header" -X GET -o "$body_out" -w '%{http_code}' \
      --max-time 60 "${base_url}${path}"); then
      status=000
    else
      status=$response
    fi
    case "$status" in
      200|201|202|204) break ;;
      500|502|503|000)
        if [ "$attempt" -lt "$max_attempts" ]; then
          sleep $((attempt * 2))
          attempt=$((attempt + 1))
          continue
        fi
        ;;
    esac
    break
  done
  end_ms=$(now_ms)
  latency=$((end_ms - start_ms))
  printf '%s\n' "$latency" >>"$lat_file"
  case "$status" in
    200|201|202|204|101) ;;
    *) printf 'GET %s %s\n' "$path" "$status" >>"$err_file" ;;
  esac
}

wait_queue_label() {
  label=$1
  attempts=${2:-40}
  while [ "$attempts" -gt 0 ]; do
    body=$(curl -sS -H "$host_header" \
      "${base_url}/stack/queue/dequeue-verify?label=${label}" 2>/dev/null || echo '{}')
    processed=$(printf '%s' "$body" | jq -r '.processed | if . == null then "false" else tostring end')
    if [ "$processed" = "true" ]; then
      return 0
    fi
    sleep 0.5
    attempts=$((attempts - 1))
  done
  return 1
}

# Wait for mega-checkout bindings (including queue fan-out) to align.
# Timeout in seconds: second argument, else STRESS_BINDINGS_ALIGN_SEC (default 40).
wait_bindings_aligned() {
  order_id=$1
  timeout_sec=${2:-${STRESS_BINDINGS_ALIGN_SEC:-40}}
  attempts=$((timeout_sec * 2))
  while [ "$attempts" -gt 0 ]; do
    body=$(curl -sS -H "$host_header" \
      "${base_url}/stack/scenario/verify?order_id=${order_id}" 2>/dev/null || echo '{}')
    aligned=$(printf '%s' "$body" | jq -r '.bindings_aligned | if . == null then "false" else tostring end')
    if [ "$aligned" = "true" ]; then
      printf '%s' "$body"
      return 0
    fi
    sleep 0.5
    attempts=$((attempts - 1))
  done
  return 1
}

run_stack_phase() {
  label=$1
  concurrency=$2
  path=$3
  method=${4:-GET}
  body=${5:-}
  lat_file=$6
  err_file=$7
  i=1
  while [ "$i" -le "$concurrency" ]; do
    stress_request "$path" "$method" "$body" "$lat_file" "$err_file" &
    i=$((i + 1))
  done
  wait
  printf 'completed %s concurrency=%s\n' "$label" "$concurrency"
}

wait_for_ready() (
  base=${1:-$base_url}
  attempts=${2:-120}
  while [ "$attempts" -gt 0 ]; do
    ready=$(curl -fsS -o /dev/null -w '%{http_code}' "${base}/health/ready" 2>/dev/null || echo 000)
    if [ "$ready" = "200" ]; then
      return 0
    fi
    sleep 1
    attempts=$((attempts - 1))
  done
  return 1
)

wait_for_worker_ready() (
  attempts=${1:-90}
  stable=0
  while [ "$attempts" -gt 0 ]; do
    stack=$(curl -sS -H "$host_header" -o /dev/null -w '%{http_code}' \
      "${base_url}/stack/http/ping" 2>/dev/null || echo 000)
    if [ "$stack" = "200" ]; then
      stable=$((stable + 1))
      if [ "$stable" -ge 2 ]; then
        return 0
      fi
    else
      stable=0
    fi
    sleep 1
    attempts=$((attempts - 1))
  done
  return 1
)

run_duration_concurrent() {
  label=$1
  concurrency=$2
  duration_sec=$3
  path=$4
  method=${5:-GET}
  body=${6:-}
  lat_file=$7
  err_file=$8
  end_epoch=$(($(date +%s) + duration_sec))
  i=1
  while [ "$i" -le "$concurrency" ]; do
    (
      while [ "$(date +%s)" -lt "$end_epoch" ]; do
        stress_request "$path" "$method" "$body" "$lat_file" "$err_file"
      done
    ) &
    i=$((i + 1))
  done
  wait
  printf 'completed %s concurrency=%s duration=%ss\n' "$label" "$concurrency" "$duration_sec"
}

run_rate_load() {
  label=$1
  rate_per_sec=$2
  duration_sec=$3
  path=$4
  method=${5:-GET}
  body=${6:-}
  lat_file=$7
  err_file=$8
  end_epoch=$(($(date +%s) + duration_sec))
  interval_us=$((1000000 / rate_per_sec))
  sent=0
  while [ "$(date +%s)" -lt "$end_epoch" ]; do
    stress_request "$path" "$method" "$body" "$lat_file" "$err_file" &
    sent=$((sent + 1))
    if [ "$interval_us" -gt 0 ]; then
      python3 -c "import time; time.sleep(${interval_us} / 1_000_000)"
    fi
  done
  wait
  printf 'completed %s rate=%s/s duration=%ss sent~=%s\n' "$label" "$rate_per_sec" "$duration_sec" "$sent"
}
