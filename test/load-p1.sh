#!/bin/sh
# Measure a repeatable local mixed-profile envelope without claiming a universal SLA.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
workerd=${OPEN_COMPUTE_TEST_WORKERD:-"$root/.temp/runtime-cache/v1.20260826.1/workerd"}
if [ ! -f "$workerd" ]; then
  echo "OPEN_COMPUTE_TEST_WORKERD is missing; the P1 load Gate refuses to skip" >&2
  exit 1
fi
export OPEN_COMPUTE_TEST_WORKERD="$workerd"
profile=""
iterations=3
seed=1701
while [ "$#" -gt 0 ]; do
  case "$1" in
    --profile) profile=$2; shift 2 ;;
    --iterations) iterations=$2; shift 2 ;;
    --seed) seed=$2; shift 2 ;;
    *) echo "usage: $0 --profile mixed [--iterations POSITIVE] [--seed INTEGER]" >&2; exit 2 ;;
  esac
done
case "$profile" in mixed) ;; *) echo "only --profile mixed is supported" >&2; exit 2 ;; esac
case "$iterations" in ''|*[!0-9]*|0) echo "iterations must be positive" >&2; exit 2 ;; esac
case "$seed" in ''|*[!0-9]*) echo "seed must be an integer" >&2; exit 2 ;; esac

result_dir="$root/target/p1-results/load"
mkdir -p "$result_dir"
durations="$result_dir/durations.txt"
time_log="$result_dir/time.log"
last_log="$result_dir/last-events.log"
capacity_log="$result_dir/capacity.ndjson"
: >"$durations"
: >"$time_log"
: >"$last_log"
: >"$capacity_log"
start=$(/bin/date +%s)
round=1
max_rss_bytes=0
cd "$root"
while [ "$round" -le "$iterations" ]; do
  round_start=$(/bin/date +%s)
  iteration_log="$result_dir/iteration-$round.log"
  if ! /usr/bin/time -l cargo test -p open-compute-service --features test-support \
    --test p0_exit_gate -- --test-threads=1 --nocapture >"$iteration_log" 2>"$time_log"; then
    /bin/cat "$iteration_log" >&2
    /bin/cat "$time_log" >&2
    exit 1
  fi
  capacity=$(/usr/bin/grep 'P1_CAPACITY ' "$iteration_log" | /usr/bin/tail -n 1 | /usr/bin/sed 's/.*P1_CAPACITY //')
  if [ -z "$capacity" ]; then
    echo "P1 combined Gate did not emit capacity evidence" >&2
    exit 1
  fi
  printf '%s\n' "$capacity" >>"$capacity_log"
  round_finish=$(/bin/date +%s)
  printf '%s\n' "$((round_finish - round_start))" >>"$durations"
  rss=$(/usr/bin/awk '/maximum resident set size/ { value=$1 } END { print value + 0 }' "$time_log")
  if [ "$rss" -gt "$max_rss_bytes" ]; then max_rss_bytes=$rss; fi
  /usr/bin/tail -n 200 "$iteration_log" >"$last_log"
  /bin/rm "$iteration_log"
  round=$((round + 1))
done

crash_start=$(/bin/date +%s)
if ! cargo test -p open-compute-service --features test-support \
  --test p1_crash_process -- --test-threads=1 >>"$last_log" 2>&1; then
  /bin/cat "$last_log" >&2
  exit 1
fi
crash_finish=$(/bin/date +%s)
crash_recovery_gate_seconds=$((crash_finish - crash_start))
/usr/bin/sort -n "$durations" >"$result_dir/durations.sorted"
percentile() {
  percentile_value=$1
  count=$2
  rank=$(((count * percentile_value + 99) / 100))
  /usr/bin/awk -v rank="$rank" 'NR == rank { print; exit }' "$result_dir/durations.sorted"
}
p50=$(percentile 50 "$iterations")
p95=$(percentile 95 "$iterations")
p99=$(percentile 99 "$iterations")
request_samples=$(/usr/bin/sed -n 's/.*"samples":\([0-9][0-9]*\).*/\1/p' "$capacity_log" | /usr/bin/awk '{ total += $1 } END { print total + 0 }')
request_p50=$(/usr/bin/sed -n 's/.*"p50_ms":\([0-9.][0-9.]*\).*/\1/p' "$capacity_log" | /usr/bin/sort -nr | /usr/bin/awk 'NR == 1 { print; exit }')
request_p95=$(/usr/bin/sed -n 's/.*"p95_ms":\([0-9.][0-9.]*\).*/\1/p' "$capacity_log" | /usr/bin/sort -nr | /usr/bin/awk 'NR == 1 { print; exit }')
request_p99=$(/usr/bin/sed -n 's/.*"p99_ms":\([0-9.][0-9.]*\).*/\1/p' "$capacity_log" | /usr/bin/sort -nr | /usr/bin/awk 'NR == 1 { print; exit }')
finish=$(/bin/date +%s)
revision=$(git rev-parse HEAD)
workerd_lock_sha256=$(/usr/bin/shasum -a 256 "$root/packages/runtime/workerd.lock.json" | /usr/bin/awk '{print $1}')
config_sha256=$(/usr/bin/shasum -a 256 "$root/share/default-config.toml" | /usr/bin/awk '{print $1}')
host=$(/usr/bin/uname -srm | /usr/bin/tr -c 'A-Za-z0-9_.-' '_')
result="$result_dir/result.json"
printf '{"schema_version":1,"profile":"mixed","scope":"combined_p0_product_saturation_and_p1_process_recovery_gates","revision":"%s","workerd_lock_sha256":"%s","default_config_sha256":"%s","host":"%s","seed":%s,"iterations":%s,"elapsed_seconds":%s,"iteration_p50_seconds":%s,"iteration_p95_seconds":%s,"iteration_p99_seconds":%s,"request_samples":%s,"request_worst_iteration_p50_ms":%s,"request_worst_iteration_p95_ms":%s,"request_worst_iteration_p99_ms":%s,"capacity_evidence":"capacity.ndjson","max_runner_rss_bytes":%s,"process_recovery_gate_seconds":%s,"verdict":"pass"}\n' \
  "$revision" "$workerd_lock_sha256" "$config_sha256" "$host" "$seed" "$iterations" \
  "$((finish - start))" "$p50" "$p95" "$p99" "$request_samples" "$request_p50" \
  "$request_p95" "$request_p99" "$max_rss_bytes" "$crash_recovery_gate_seconds" \
  >"$result"
/bin/cat "$result"
