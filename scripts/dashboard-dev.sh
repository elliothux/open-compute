#!/bin/sh
# Serve the Dashboard with Vite HMR against an existing debug ocd build.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
ocd_bin=${OPEN_COMPUTE_OCD_BIN:-$root/target/debug/ocd}
env_file="$root/scripts/config/dev.env"
run_dir="$root/.temp/dashboard-dev"
# Keep the macOS Unix socket path within sockaddr_un's 103-byte limit.
# Keep development authority isolated from the retired pre-multi-instance layout.
scope_base="$root/.temp/od"
ocd_root="$scope_base/user"
ocd_log="$run_dir/ocd.log"
ocd_pid_file="$run_dir/ocd.pid"
api_origin=http://127.0.0.1:18788
dev_port=${OPEN_COMPUTE_DASHBOARD_DEV_PORT:-5173}
dev_origin="http://127.0.0.1:$dev_port"
ocd_pid=
vite_pid=

fail() {
  echo "open-compute dashboard dev: $*" >&2
  exit 1
}

instances_ready() {
  [ "$("$ocd_bin" --no-update-check instances --json 2>/dev/null | grep -o '"state":"running"' | wc -l | tr -d ' ')" -eq 2 ]
}

process_running() {
  process_state=$(ps -p "$1" -o state= 2>/dev/null | tr -d ' ' || true)
  [ -n "$process_state" ] && [ "${process_state#Z}" = "$process_state" ]
}

wait_for_exit() {
  child_pid=$1
  attempts=0
  while process_running "$child_pid" && [ "$attempts" -lt 30 ]; do
    attempts=$((attempts + 1))
    sleep 0.1
  done
  forced=false
  if process_running "$child_pid"; then
    forced=true
    kill -KILL "$child_pid" 2>/dev/null || true
  fi
  wait "$child_pid" 2>/dev/null || true
  [ "$forced" = false ]
}

stop_children() {
  # Do not let a second Ctrl-C interrupt cleanup and orphan the scope owner.
  trap - EXIT HUP TERM
  trap '' INT
  echo "open-compute dashboard dev: stopping Vite and ocd"
  [ -z "${vite_pid:-}" ] || kill "$vite_pid" 2>/dev/null || true
  [ -z "${ocd_pid:-}" ] || kill "$ocd_pid" 2>/dev/null || true
  if [ -n "${vite_pid:-}" ]; then
    wait_for_exit "$vite_pid" || true
    vite_pid=
  fi
  if [ -n "${ocd_pid:-}" ]; then
    wait_for_exit "$ocd_pid" || true
    ocd_pid=
  fi
  rm -f "$ocd_pid_file"
}

stop_recorded_ocd() {
  [ -f "$ocd_pid_file" ] || return 0
  old_pid=$(sed -n '1p' "$ocd_pid_file")
  case "$old_pid" in
    ''|*[!0-9]*) rm -f "$ocd_pid_file"; return 0 ;;
  esac
  if ! process_running "$old_pid"; then
    rm -f "$ocd_pid_file"
    return 0
  fi
  old_command=$(ps -p "$old_pid" -o command= 2>/dev/null || true)
  case "$old_command" in
    "$ocd_bin --no-update-check run"*)
      echo "open-compute dashboard dev: stopping leftover ocd process $old_pid"
      kill "$old_pid" 2>/dev/null || true
      wait_for_exit "$old_pid" || true
      rm -f "$ocd_pid_file"
      ;;
    *) fail "$ocd_pid_file points to unrelated process $old_pid" ;;
  esac
}

trap stop_children EXIT
trap 'stop_children; exit 129' HUP
trap 'stop_children; exit 130' INT
trap 'stop_children; exit 143' TERM

case "$ocd_bin" in
  /*) ;;
  *) fail "OPEN_COMPUTE_OCD_BIN must be absolute" ;;
esac
[ -f "$env_file" ] || fail "checked-in development environment is missing"

# A normal Cargo build writes the same target/debug/ocd path but ignores the
# isolated development root. Restore the required variant only when needed;
# subsequent dashboard runs reuse it and Vite owns frontend rebuilds.
if [ ! -x "$ocd_bin" ] || ! LC_ALL=C grep -a -q 'OPEN_COMPUTE_TEST_OCD_ROOT' "$ocd_bin"; then
  echo "open-compute dashboard dev: preparing isolated OCD build"
  (
    cd "$root"
    bun run build
    cargo build -p open-compute-service --features test-support --bin ocd
  ) || fail "failed to build the isolated development OCD"
fi
LC_ALL=C grep -a -q 'OPEN_COMPUTE_TEST_OCD_ROOT' "$ocd_bin" || \
  fail "$ocd_bin does not include the required test-support feature"

set -a
. "$env_file"
set +a
export OPEN_COMPUTE_DASHBOARD_DEV_API_ORIGIN="$api_origin"
export OPEN_COMPUTE_DASHBOARD_DEV_PORT="$dev_port"
export OPEN_COMPUTE_TEST_OCD_ROOT="$scope_base"

umask 077
mkdir -p "$run_dir" "$ocd_root"
stop_recorded_ocd
manifest="$ocd_root/ocd.toml"
if [ ! -e "$manifest" ]; then
  (
    set -C
    printf '[server]\npublic_bind = "127.0.0.1:18788"\nadmin_auth = { env = "OPEN_COMPUTE_ADMIN_TOKEN" }\n' >"$manifest"
  ) || fail "failed to create the dashboard development manifest"
fi
# Besides validating the registry, this opens and releases the offline scope
# lock before `run`; macOS can otherwise reject the first post-interrupt lock.
"$ocd_bin" --no-update-check instances --json >/dev/null 2>&1 || \
  fail "dashboard development OCD scope is unavailable"
: >"$ocd_log"
"$ocd_bin" --no-update-check run >"$ocd_log" 2>&1 &
ocd_pid=$!
printf '%s\n' "$ocd_pid" >"$ocd_pid_file"

attempt=0
until "$ocd_bin" --no-update-check status --json 2>/dev/null | grep -F '"state":"running"' >/dev/null; do
  if ! kill -0 "$ocd_pid" 2>/dev/null; then
    wait "$ocd_pid" 2>/dev/null || true
    ocd_pid=
    sed -n '1,160p' "$ocd_log" >&2
    fail "ocd exited before readiness"
  fi
  attempt=$((attempt + 1))
  if [ "$attempt" -ge 600 ]; then
    sed -n '1,160p' "$ocd_log" >&2
    fail "ocd did not become ready within 60 seconds"
  fi
  sleep 0.1
done

for instance_name in dashboard-dev dashboard-preview; do
  instance_root="$ocd_root/instances/$instance_name"
  instance_config="$instance_root/compute.toml"
  if [ ! -e "$instance_config" ]; then
    "$ocd_bin" --no-update-check instance setup \
      --name "$instance_name" \
      --config "$instance_config" \
      --data-dir "$instance_root/data" \
      --yes
  fi
done

attempt=0
until curl --fail --silent --show-error --max-time 1 "$api_origin/health/ready" >/dev/null 2>&1 && instances_ready; do
  if ! kill -0 "$ocd_pid" 2>/dev/null; then
    wait "$ocd_pid" 2>/dev/null || true
    ocd_pid=
    sed -n '1,160p' "$ocd_log" >&2
    fail "ocd exited before readiness"
  fi
  attempt=$((attempt + 1))
  if [ "$attempt" -ge 600 ]; then
    sed -n '1,160p' "$ocd_log" >&2
    fail "ocd did not become ready within 60 seconds"
  fi
  sleep 0.1
done
if ! kill -0 "$ocd_pid" 2>/dev/null; then
  wait "$ocd_pid" 2>/dev/null || true
  ocd_pid=
  sed -n '1,160p' "$ocd_log" >&2
  fail "ocd exited while another process answered on $api_origin"
fi

login_path=/operator/login

echo "open-compute dashboard dev: $dev_origin$login_path"
echo "open-compute dashboard dev: admin token $OPEN_COMPUTE_ADMIN_TOKEN"
echo "open-compute dashboard dev: instances dashboard-dev, dashboard-preview"
echo "open-compute dashboard dev: OCD log $ocd_log"
cd "$root/apps/dashboard"
bunx vite --host 127.0.0.1 --strictPort --open "$login_path" &
vite_pid=$!
wait "$vite_pid"
