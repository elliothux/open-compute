#!/bin/sh
# Run ocd against isolated repository-local state and the direct Local backend.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
config="$root/scripts/config/dev-test.toml"
env_file="$root/scripts/config/dev.env"
run_dir="$root/.temp/dev-test"
ocd_log="$run_dir/ocd-run.log"
ocd_pid=

fail() {
  echo "open-compute dev-test: $*" >&2
  exit 1
}

stop_ocd() {
  if [ -n "${ocd_pid:-}" ]; then
    kill "$ocd_pid" 2>/dev/null || true
    wait "$ocd_pid" 2>/dev/null || true
    ocd_pid=
  fi
}

trap stop_ocd EXIT
trap 'stop_ocd; exit 129' HUP
trap 'stop_ocd; exit 130' INT
trap 'stop_ocd; exit 143' TERM

[ -f "$config" ] || fail "checked-in test config is missing"
[ -f "$env_file" ] || fail "checked-in development environment is missing"
set -a
. "$env_file"
set +a

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
  archive=${OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE:-}
  case "$archive" in
    /*) ;;
    *) fail "set OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE when no ocd binary is supplied" ;;
  esac
  [ -f "$archive" ] || fail "the pinned build archive is missing"
  export OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE="$archive"
  printf '%s\n' cargo
}

run_ocd() {
  executable=$1
  shift
  if [ "$executable" = cargo ]; then
    (cd "$root" && exec cargo run -p open-compute-service --bin ocd -- --config "$config" "$@")
  else
    exec "$executable" --config "$config" "$@"
  fi
}

executable=$(resolve_ocd)
if [ "${1:-}" != smoke ]; then
  if [ "$#" -eq 0 ]; then
    set -- run
  fi
  run_ocd "$executable" "$@"
  exit $?
fi

shift
[ "$#" -eq 0 ] || fail "smoke does not accept extra arguments"
command -v curl >/dev/null 2>&1 || fail "curl is required for smoke"
umask 077
mkdir -p "$run_dir"
: >"$ocd_log"
run_ocd "$executable" run >"$ocd_log" 2>&1 &
ocd_pid=$!

attempt=0
while [ "$attempt" -lt 200 ]; do
  if ! kill -0 "$ocd_pid" 2>/dev/null; then
    wait "$ocd_pid" 2>/dev/null || true
    ocd_pid=
    sed -n '1,160p' "$ocd_log" >&2
    fail "ocd exited before readiness"
  fi
  if curl --fail --silent --show-error --max-time 1 \
    http://127.0.0.1:18787/health/ready >/dev/null 2>&1; then
    stop_ocd
    echo "open-compute dev-test: Local readiness smoke passed"
    exit 0
  fi
  attempt=$((attempt + 1))
  sleep 0.1
done

sed -n '1,160p' "$ocd_log" >&2
fail "ocd did not become ready"
