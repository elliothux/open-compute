#!/bin/sh
# Run a local bounded P1 mixed soak; no CI, upload, or remote threshold service.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
workerd=${OPEN_COMPUTE_TEST_WORKERD:-}
archive=${OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE:-}
case "$workerd" in /*) ;; *) echo "OPEN_COMPUTE_TEST_WORKERD must name an existing absolute path" >&2; exit 1 ;; esac
case "$archive" in /*) ;; *) echo "OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE must name an existing absolute path" >&2; exit 1 ;; esac
[ -f "$workerd" ] || { echo "OPEN_COMPUTE_TEST_WORKERD is missing" >&2; exit 1; }
[ -f "$archive" ] || { echo "OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE is missing" >&2; exit 1; }
duration=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --duration) duration=$2; shift 2 ;;
    *) echo "usage: $0 --duration 10m|1h|24h" >&2; exit 2 ;;
  esac
done
case "$duration" in
  10m) seconds=600 ;;
  1h) seconds=3600 ;;
  24h) seconds=86400 ;;
  *) echo "usage: $0 --duration 10m|1h|24h" >&2; exit 2 ;;
esac

run_id=$(/bin/date -u +%Y%m%dT%H%M%SZ)-$$
result_parent="$root/.temp/soak-p1"
result_dir="$result_parent/$run_id"
mkdir -p "$result_parent"
mkdir "$result_dir"
event_log="$result_dir/last-events.log"
: >"$event_log"
start=$(/bin/date +%s)
deadline=$((start + seconds))
iterations=0
cd "$root"
while [ "$iterations" -eq 0 ] || [ "$(/bin/date +%s)" -lt "$deadline" ]; do
  iteration_log="$result_dir/iteration.log"
  : >"$iteration_log"
  if ! cargo test -p open-compute-service --features test-support \
    --test p0_exit_gate -- --test-threads=1 >>"$iteration_log" 2>&1 \
    || ! cargo test -p open-compute-service --features test-support \
      --test p1_crash_process -- --test-threads=1 >>"$iteration_log" 2>&1 \
    || ! cargo test -p open-compute-service --features test-support \
      --test p1_reliability -- --test-threads=1 >>"$iteration_log" 2>&1; then
    /usr/bin/tail -n 400 "$iteration_log" >"$event_log"
    /bin/cat "$event_log" >&2
    exit 1
  fi
  /usr/bin/tail -n 400 "$iteration_log" >"$event_log"
  /bin/rm "$iteration_log"
  iterations=$((iterations + 1))
done
finish=$(/bin/date +%s)
revision=$(git rev-parse HEAD)
workerd_lock_sha256=$(/usr/bin/shasum -a 256 "$root/packages/runtime/workerd.lock.json" | /usr/bin/awk '{print $1}')
config_sha256=$(/usr/bin/shasum -a 256 "$root/share/default-config.toml" | /usr/bin/awk '{print $1}')
host=$(/usr/bin/uname -srm | /usr/bin/tr -c 'A-Za-z0-9_.-' '_')
result="$result_dir/result.json"
printf '{"schema_version":1,"profile":"mixed","fault_schedule":"fixed_p0_combined_then_platformd_sigkill_then_reliability","run_id":"%s","duration":"%s","revision":"%s","workerd_lock_sha256":"%s","default_config_sha256":"%s","host":"%s","elapsed_seconds":%s,"iterations":%s,"bounded_event_lines":400,"verdict":"pass"}\n' \
  "$run_id" "$duration" "$revision" "$workerd_lock_sha256" "$config_sha256" "$host" \
  "$((finish - start))" "$iterations" >"$result"
/bin/cat "$result"
