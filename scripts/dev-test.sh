#!/bin/sh
# Start ocd against isolated repository-local test state and a local S3 fixture.
#
# Usage:
#   ./scripts/dev-test.sh              # foreground ocd (S3 fixture in background)
#   ./scripts/dev-test.sh smoke        # start, probe endpoints, stop
#   ./scripts/dev-test.sh run ...      # pass ocd subcommands (default: run)
#
# Environment:
#   OPEN_COMPUTE_DEV_S3                mock (default) or rclone
#   OPEN_COMPUTE_OCD_BIN               optional absolute path to a built ocd binary
#   OPEN_COMPUTE_S3_FIXTURE_BIN        optional absolute path to open-compute-s3-fixture
#   OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE required only when falling back to cargo run
#   OPEN_COMPUTE_ADMIN_TOKEN           defaults to dev-admin-token
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
data_root="$root/.temp/dev-test"
s3_root="$data_root/s3"
platform_data="$data_root/platform"
config="$data_root/dev-config.toml"
s3_log="$data_root/s3-fixture.log"
rclone_log="$data_root/rclone-s3.log"
ocd_log="$data_root/ocd-run.log"
s3_backend=${OPEN_COMPUTE_DEV_S3:-mock}
s3_pid=
rclone_pid=
ocd_pid=
smoke=0
s3_endpoint=

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

stop_s3() {
  if [ -n "${s3_pid:-}" ]; then
    kill "$s3_pid" 2>/dev/null || true
    wait "$s3_pid" 2>/dev/null || true
    s3_pid=
  fi
}

stop_rclone() {
  if [ -n "${rclone_pid:-}" ]; then
    kill "$rclone_pid" 2>/dev/null || true
    wait "$rclone_pid" 2>/dev/null || true
    rclone_pid=
  fi
}

cleanup() {
  stop_ocd
  stop_s3
  stop_rclone
}

trap cleanup EXIT
trap 'cleanup; exit 129' HUP
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM

command -v python3 >/dev/null 2>&1 || fail "python3 is required"
command -v curl >/dev/null 2>&1 || fail "curl is required"

if [ "${1:-}" = "smoke" ]; then
  smoke=1
  shift
fi

case "$s3_backend" in
  mock|rclone) ;;
  *) fail "OPEN_COMPUTE_DEV_S3 must be mock or rclone" ;;
esac

if [ "$s3_backend" = "rclone" ]; then
  command -v rclone >/dev/null 2>&1 || fail "rclone with 'serve s3' support is required"
fi

umask 077
mkdir -p "$s3_root/open-compute" "$platform_data"
chmod 700 "$data_root" "$s3_root" "$s3_root/open-compute" "$platform_data"

export TMPDIR="$platform_data"
export S3_ACCESS_KEY_ID=open-compute-dev
export S3_SECRET_ACCESS_KEY=open-compute-dev-secret
: "${OPEN_COMPUTE_ADMIN_TOKEN:=dev-admin-token}"
: "${OPEN_COMPUTE_DEPLOYER_TOKEN:=dev-deployer-token}"
: "${OPEN_COMPUTE_READ_ONLY_TOKEN:=dev-read-only-token}"
export OPEN_COMPUTE_ADMIN_TOKEN
export OPEN_COMPUTE_DEPLOYER_TOKEN
export OPEN_COMPUTE_READ_ONLY_TOKEN

write_config() {
  endpoint=$1
  config_tmp="$config.tmp.$$"
  cat >"$config_tmp" <<EOF
[server]
public_bind = "127.0.0.1:8787"
admin_auth = { env = "OPEN_COMPUTE_ADMIN_TOKEN" }
deployer_auth = { env = "OPEN_COMPUTE_DEPLOYER_TOKEN" }
read_only_auth = { env = "OPEN_COMPUTE_READ_ONLY_TOKEN" }

[storage]
data_dir = "$platform_data"
master_key_file = "$platform_data/keys/master.key"

[s3]
endpoint = "$endpoint"
region = "auto"
bucket = "open-compute"
force_path_style = true
access_key_id_env = "S3_ACCESS_KEY_ID"
secret_access_key_env = "S3_SECRET_ACCESS_KEY"
prefix = "system/"

[runtime]
startup_timeout_ms = 20000
shutdown_grace_ms = 10000

[durable_objects]
# Keep local product testing writable while preserving a hard free-space floor.
disk_high_watermark_percent = 98
disk_stop_writes_percent = 99

[dashboard]
enabled = true
EOF
  mv "$config_tmp" "$config"
}

resolve_s3_fixture_bin() {
  if [ -n "${OPEN_COMPUTE_S3_FIXTURE_BIN:-}" ]; then
    case "$OPEN_COMPUTE_S3_FIXTURE_BIN" in
      /*) ;;
      *) fail "OPEN_COMPUTE_S3_FIXTURE_BIN must be an absolute path" ;;
    esac
    [ -x "$OPEN_COMPUTE_S3_FIXTURE_BIN" ] || fail "OPEN_COMPUTE_S3_FIXTURE_BIN is missing or not executable"
    printf '%s\n' "$OPEN_COMPUTE_S3_FIXTURE_BIN"
    return 0
  fi

  candidate="$root/target/debug/open-compute-s3-fixture"
  if [ -x "$candidate" ]; then
    printf '%s\n' "$candidate"
    return 0
  fi

  command -v cargo >/dev/null 2>&1 || fail "build open-compute-s3-fixture or set OPEN_COMPUTE_S3_FIXTURE_BIN"
  printf '%s\n' "cargo-run-s3-fixture"
}

start_mock_s3() {
  fixture_bin=$(resolve_s3_fixture_bin)
  : >"$s3_log"

  if [ "$fixture_bin" = "cargo-run-s3-fixture" ]; then
    (
      cd "$root"
      cargo run -p open-compute-artifacts --bin open-compute-s3-fixture --features test-support
    ) >>"$s3_log" 2>&1 &
  else
    "$fixture_bin" >>"$s3_log" 2>&1 &
  fi
  s3_pid=$!

  attempt=0
  while [ "$attempt" -lt 50 ]; do
    if ! kill -0 "$s3_pid" 2>/dev/null; then
      wait "$s3_pid" 2>/dev/null || true
      s3_pid=
      sed -n '1,120p' "$s3_log" >&2
      fail "open-compute-s3-fixture exited before becoming ready"
    fi
    s3_endpoint=$(
      python3 - "$s3_log" <<'PY'
import json
import sys

path = sys.argv[1]
try:
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if not line.startswith("{"):
                continue
            payload = json.loads(line)
            endpoint = payload.get("endpoint")
            if isinstance(endpoint, str) and endpoint:
                print(endpoint)
                raise SystemExit(0)
except OSError:
    pass
PY
    ) || s3_endpoint=
    if [ -n "$s3_endpoint" ]; then
      return 0
    fi
    attempt=$((attempt + 1))
    sleep 0.1
  done
  sed -n '1,120p' "$s3_log" >&2
  fail "open-compute-s3-fixture did not publish its endpoint"
}

start_rclone() {
  s3_endpoint=http://127.0.0.1:9000
  : >"$rclone_log"
  RCLONE_AUTH_KEY='"open-compute-dev,open-compute-dev-secret"' rclone serve s3 \
    --addr 127.0.0.1:9000 \
    ":local:$s3_root" \
    >>"$rclone_log" 2>&1 &
  rclone_pid=$!

  attempt=0
  while [ "$attempt" -lt 50 ]; do
    if ! kill -0 "$rclone_pid" 2>/dev/null; then
      wait "$rclone_pid" 2>/dev/null || true
      rclone_pid=
      sed -n '1,120p' "$rclone_log" >&2
      fail "rclone S3 server exited before becoming ready"
    fi
    if python3 -c 'import socket; s = socket.create_connection(("127.0.0.1", 9000), 0.2); s.close()' 2>/dev/null; then
      return 0
    fi
    attempt=$((attempt + 1))
    sleep 0.1
  done
  fail "rclone S3 server did not listen on 127.0.0.1:9000"
}

start_s3() {
  case "$s3_backend" in
    mock) start_mock_s3 ;;
    rclone) start_rclone ;;
  esac
  write_config "$s3_endpoint"
}

resolve_ocd_bin() {
  if [ -n "${OPEN_COMPUTE_OCD_BIN:-}" ]; then
    case "$OPEN_COMPUTE_OCD_BIN" in
      /*) ;;
      *) fail "OPEN_COMPUTE_OCD_BIN must be an absolute path" ;;
    esac
    [ -x "$OPEN_COMPUTE_OCD_BIN" ] || fail "OPEN_COMPUTE_OCD_BIN is missing or not executable"
    printf '%s\n' "$OPEN_COMPUTE_OCD_BIN"
    return 0
  fi

  for candidate in "$root/target/release/ocd" "$root/target/debug/ocd"; do
    if [ -x "$candidate" ]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done

  command -v cargo >/dev/null 2>&1 || fail "set OPEN_COMPUTE_OCD_BIN or build target/debug/ocd first"
  archive=${OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE:-}
  case "$archive" in
    /*) ;;
    *) fail "set OPEN_COMPUTE_OCD_BIN or OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE for cargo run" ;;
  esac
  [ -f "$archive" ] || fail "the pinned build archive is missing"
  export OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE="$archive"
  printf '%s\n' "cargo-run"
}

run_ocd() {
  ocd_bin=$(resolve_ocd_bin)
  : >"$ocd_log"

  if [ "$ocd_bin" = "cargo-run" ]; then
    (
      cd "$root"
      cargo run -p open-compute-service --bin ocd -- --config "$config" "$@"
    ) >>"$ocd_log" 2>&1 &
  else
    "$ocd_bin" --config "$config" "$@" >>"$ocd_log" 2>&1 &
  fi
  ocd_pid=$!
}

wait_for_health() {
  attempt=0
  while [ "$attempt" -lt 90 ]; do
    if curl -sf --max-time 1 http://127.0.0.1:8787/health/live >/dev/null 2>&1; then
      return 0
    fi
    if [ -n "${ocd_pid:-}" ] && ! kill -0 "$ocd_pid" 2>/dev/null; then
      wait "$ocd_pid" 2>/dev/null || true
      ocd_pid=
      sed -n '1,80p' "$ocd_log" >&2
      if [ "$s3_backend" = "rclone" ] && grep -q 'R2_PROVIDER_UNAVAILABLE' "$ocd_log" 2>/dev/null; then
        echo "open-compute dev-test: rclone serve s3 does not satisfy the R2 capability preflight; use OPEN_COMPUTE_DEV_S3=mock" >&2
      fi
      fail "ocd exited before /health/live became ready"
    fi
    attempt=$((attempt + 1))
    sleep 1
  done
  sed -n '1,80p' "$ocd_log" >&2
  fail "timed out waiting for http://127.0.0.1:8787/health/live"
}

wait_for_dashboard() {
  attempt=0
  while [ "$attempt" -lt 120 ]; do
    status=$(curl -s -o /dev/null -w '%{http_code}' --max-time 2 http://127.0.0.1:8787/operator/ || printf '%s' "000")
    if [ "$status" = "200" ]; then
      return 0
    fi
    if [ -n "${ocd_pid:-}" ] && ! kill -0 "$ocd_pid" 2>/dev/null; then
      wait "$ocd_pid" 2>/dev/null || true
      ocd_pid=
      sed -n '1,80p' "$ocd_log" >&2
      fail "ocd exited before /operator/ became ready"
    fi
    attempt=$((attempt + 1))
    sleep 1
  done
  sed -n '1,80p' "$ocd_log" >&2
  fail "timed out waiting for http://127.0.0.1:8787/operator/"
}

probe_endpoint() {
  name=$1
  url=$2
  shift 2
  status=$(curl -s -o /dev/null -w '%{http_code}' --max-time 2 "$@" "$url" || printf '%s' "000")
  echo "open-compute dev-test: $name $status $url" >&2
  printf '%s' "$status"
}

run_smoke() {
  start_s3
  run_ocd run
  wait_for_health
  wait_for_dashboard

  live=$(probe_endpoint live http://127.0.0.1:8787/health/live)
  ready=$(probe_endpoint ready http://127.0.0.1:8787/health/ready)
  dashboard=$(probe_endpoint dashboard http://127.0.0.1:8787/operator/)
  capabilities=$(
    probe_endpoint capabilities http://127.0.0.1:8787/client/v4/open-compute/capabilities \
      -H "Authorization: Bearer $OPEN_COMPUTE_ADMIN_TOKEN"
  )

  stop_ocd

  if [ "$live" != "200" ] || [ "$capabilities" != "200" ] || [ "$dashboard" != "200" ]; then
    sed -n '1,80p' "$ocd_log" >&2
    fail "smoke failed (live=$live capabilities=$capabilities ready=$ready dashboard=$dashboard)"
  fi

  echo "open-compute dev-test: smoke passed"
  echo "open-compute dev-test: dashboard http://127.0.0.1:8787/operator/"
}

print_info() {
  ocd_bin=$(resolve_ocd_bin)
  echo "open-compute dev-test: S3 backend  $s3_backend"
  echo "open-compute dev-test: S3 endpoint $s3_endpoint"
  echo "open-compute dev-test: platform data $platform_data"
  echo "open-compute dev-test: config        $config"
  echo "open-compute dev-test: ocd log       $ocd_log"
  echo "open-compute dev-test: ocd binary    $ocd_bin"
  echo "open-compute dev-test: dashboard     http://127.0.0.1:8787/operator/"
  echo "open-compute dev-test: admin token   $OPEN_COMPUTE_ADMIN_TOKEN"
}

if [ "$smoke" -eq 1 ]; then
  run_smoke
  exit 0
fi

if [ "$#" -eq 0 ]; then
  set -- run
fi

start_s3
print_info

ocd_bin=$(resolve_ocd_bin)
if [ "$ocd_bin" = "cargo-run" ]; then
  trap 'cleanup; exit 129' HUP
  trap 'cleanup; exit 130' INT
  trap 'cleanup; exit 143' TERM
  cd "$root"
  exec cargo run -p open-compute-service --bin ocd -- --config "$config" "$@"
fi

trap - EXIT
trap 'stop_s3; stop_rclone; exit 129' HUP
trap 'stop_s3; stop_rclone; exit 130' INT
trap 'stop_s3; stop_rclone; exit 143' TERM
exec "$ocd_bin" --config "$config" "$@"
