#!/bin/sh
# Run a test-support daemon in a bounded repository-local OCD scope.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
state_dir=${OPEN_COMPUTE_DEV_STATE_DIR:-$root/.temp/dev-test}
scope_base=${OPEN_COMPUTE_DEV_OCD_ROOT:-$root/.temp/r1}
production_scope=${OPEN_COMPUTE_DEV_PRODUCTION_SCOPE:-0}
port=${OPEN_COMPUTE_DEV_PORT:-18787}
env_file=$root/scripts/config/dev.env
ocd_pid=

fail() {
  echo "open-compute dev-test: $*" >&2
  exit 1
}

stop_ocd() {
  if [ -n "${ocd_pid:-}" ]; then
    kill -TERM "$ocd_pid" 2>/dev/null || true
    wait "$ocd_pid" 2>/dev/null || true
    ocd_pid=
  fi
}

trap stop_ocd EXIT
trap 'stop_ocd; exit 129' HUP
trap 'stop_ocd; exit 130' INT
trap 'stop_ocd; exit 143' TERM

case "$state_dir" in
  /*) ;;
  *) fail "state root must be absolute" ;;
esac
case "$scope_base" in
  /*) ;;
  *) fail "OCD root must be absolute" ;;
esac
case "$production_scope" in
  0) ocd_root=$scope_base/user ;;
  1)
    [ -n "${OPEN_COMPUTE_OCD_BIN:-}" ] || fail "production scope requires OPEN_COMPUTE_OCD_BIN"
    [ ! -e "$scope_base" ] || fail "production OCD root already exists"
    ocd_root=$scope_base
    ;;
  *) fail "OPEN_COMPUTE_DEV_PRODUCTION_SCOPE must be 0 or 1" ;;
esac
config=$ocd_root/instances/dev/compute.toml
data=$ocd_root/instances/dev/data
case "$port" in
  ''|*[!0-9]*) fail "port must be numeric" ;;
esac
[ "$port" -ge 1 ] && [ "$port" -le 65535 ] || fail "port is out of range"
[ -f "$env_file" ] || fail "checked-in development environment is missing"
set -a
. "$env_file"
set +a
if [ "$production_scope" -eq 0 ]; then
  export OPEN_COMPUTE_TEST_OCD_ROOT=$scope_base
else
  unset OPEN_COMPUTE_TEST_OCD_ROOT
fi

resolve_ocd() {
  if [ -n "${OPEN_COMPUTE_OCD_BIN:-}" ]; then
    case "$OPEN_COMPUTE_OCD_BIN" in
      /*) ;;
      *) fail "OPEN_COMPUTE_OCD_BIN must be absolute" ;;
    esac
    [ -x "$OPEN_COMPUTE_OCD_BIN" ] || fail "OPEN_COMPUTE_OCD_BIN is not executable"
    printf '%s\n' "$OPEN_COMPUTE_OCD_BIN"
    return
  fi
  (cd "$root" && bun run build && cargo build -p open-compute-service --features test-support --bin ocd) >&2
  target_dir=${CARGO_TARGET_DIR:-$root/target}
  case "$target_dir" in
    /*) ;;
    *) target_dir=$root/$target_dir ;;
  esac
  printf '%s\n' "$target_dir/debug/ocd"
}

executable=$(resolve_ocd)
[ -x "$executable" ] || fail "test-support ocd executable is missing"
if [ "${1:-run}" != run ] && [ "${1:-}" != smoke ]; then
  exec "$executable" --no-update-check "$@"
fi
[ "$#" -le 1 ] || fail "run and smoke accept no extra arguments"

umask 077
mkdir -p "$state_dir" "$ocd_root"
manifest=$ocd_root/ocd.toml
if [ ! -e "$manifest" ]; then
  (
    set -C
    printf '[server]\npublic_bind = "127.0.0.1:%s"\nadmin_auth = { env = "OPEN_COMPUTE_ADMIN_TOKEN" }\n' "$port" > "$manifest"
  ) || fail "failed to create the scoped manifest without overwrite"
fi
ocd_log=$(mktemp "$state_dir/ocd-run.XXXXXX")
"$executable" --no-update-check run > "$ocd_log" 2>&1 &
ocd_pid=$!

attempt=0
until "$executable" --no-update-check status --json 2>/dev/null | grep -F '"state":"running"' >/dev/null; do
  kill -0 "$ocd_pid" 2>/dev/null || { sed -n '1,160p' "$ocd_log" >&2; fail "ocd exited before startup"; }
  attempt=$((attempt + 1))
  [ "$attempt" -lt 200 ] || { sed -n '1,160p' "$ocd_log" >&2; fail "ocd did not start"; }
  sleep 0.1
done

if [ ! -e "$config" ]; then
  "$executable" --no-update-check instance setup --name dev --config "$config" --data-dir "$data" --yes
fi

if [ "${1:-run}" = smoke ]; then
  command -v curl >/dev/null 2>&1 || fail "curl is required for smoke"
  attempt=0
  until curl --fail --silent --max-time 1 "http://127.0.0.1:$port/health/ready" >/dev/null; do
    kill -0 "$ocd_pid" 2>/dev/null || { sed -n '1,160p' "$ocd_log" >&2; fail "ocd exited before readiness"; }
    attempt=$((attempt + 1))
    [ "$attempt" -lt 200 ] || { sed -n '1,160p' "$ocd_log" >&2; fail "ocd did not become ready"; }
    sleep 0.1
  done
  stop_ocd
  echo "open-compute dev-test: shared-daemon readiness smoke passed"
  exit 0
fi

wait "$ocd_pid"
ocd_pid=
