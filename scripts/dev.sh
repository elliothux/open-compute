#!/bin/sh
# Run ocd with repository-local persistent development state.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
data_root="$root/.data"
s3_root="$data_root/s3"
platform_data="$data_root/platform"
config="$data_root/dev-config.toml"
rclone_log="$data_root/rclone-s3.log"
rclone_pid=

fail() {
  echo "open-compute dev: $*" >&2
  exit 1
}

stop_rclone() {
  if [ -n "${rclone_pid:-}" ]; then
    kill "$rclone_pid" 2>/dev/null || true
    wait "$rclone_pid" 2>/dev/null || true
  fi
}

trap stop_rclone EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

command -v cargo >/dev/null 2>&1 || fail "cargo is required"
command -v python3 >/dev/null 2>&1 || fail "python3 is required"
command -v rclone >/dev/null 2>&1 || fail "rclone with 'serve s3' support is required"

archive=${OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE:-}
case "$archive" in
  /*) ;;
  *) fail "set OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE to an absolute pinned .gz archive" ;;
esac
[ -f "$archive" ] || fail "the pinned build archive is missing"
export OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE="$archive"

umask 077
mkdir -p "$s3_root/open-compute" "$platform_data"
chmod 700 "$data_root" "$s3_root" "$s3_root/open-compute" "$platform_data"

# Keep platform runtime staging and persistent state inside the repository.
export TMPDIR="$platform_data"
export S3_ACCESS_KEY_ID=open-compute-dev
export S3_SECRET_ACCESS_KEY=open-compute-dev-secret
: "${OPEN_COMPUTE_ADMIN_TOKEN:=dev-admin-token}"
export OPEN_COMPUTE_ADMIN_TOKEN

config_tmp="$config.tmp.$$"
trap 'rm -f "$config_tmp"; exit 129' HUP
trap 'rm -f "$config_tmp"; exit 130' INT
trap 'rm -f "$config_tmp"; exit 143' TERM

cat >"$config_tmp" <<EOF
[server]
public_bind = "127.0.0.1:8787"
admin_auth = { env = "OPEN_COMPUTE_ADMIN_TOKEN" }

[storage]
data_dir = "$platform_data"
master_key_file = "$platform_data/keys/master.key"

[s3]
endpoint = "http://127.0.0.1:9000"
region = "auto"
bucket = "open-compute"
force_path_style = true
access_key_id_env = "S3_ACCESS_KEY_ID"
secret_access_key_env = "S3_SECRET_ACCESS_KEY"
prefix = "system/"

[runtime]
startup_timeout_ms = 20000
shutdown_grace_ms = 10000

[dashboard]
enabled = true
EOF
mv "$config_tmp" "$config"

: >"$rclone_log"
RCLONE_AUTH_KEY='"open-compute-dev,open-compute-dev-secret"' rclone serve s3 \
  --addr 127.0.0.1:9000 \
  ":local:$s3_root" \
  >"$rclone_log" 2>&1 &
rclone_pid=$!

attempt=0
while [ "$attempt" -lt 50 ]; do
  if ! kill -0 "$rclone_pid" 2>/dev/null; then
    wait "$rclone_pid" 2>/dev/null || true
    sed -n '1,120p' "$rclone_log" >&2
    fail "rclone S3 server exited before becoming ready"
  fi
  if python3 -c 'import socket; s = socket.create_connection(("127.0.0.1", 9000), 0.2); s.close()' 2>/dev/null; then
    break
  fi
  attempt=$((attempt + 1))
  sleep 0.1
done
[ "$attempt" -lt 50 ] || fail "rclone S3 server did not listen on 127.0.0.1:9000"

if [ "$#" -eq 0 ]; then
  set -- run
fi

echo "open-compute dev: S3 data      $s3_root"
echo "open-compute dev: platform data $platform_data"
echo "open-compute dev: config        $config"
echo "open-compute dev: dashboard     http://127.0.0.1:8787/operator/"
echo "open-compute dev: admin token   OPEN_COMPUTE_ADMIN_TOKEN"

cd "$root"
cargo run -p open-compute-service --bin ocd -- --config "$config" "$@"
