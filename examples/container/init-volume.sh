#!/bin/sh
# Initialize a fresh volume through the daemon's supported instance setup command.
set -eu
umask 077

OCD=/opt/open-compute/ocd
ROOT=/var/lib/open-compute
CONFIG="$ROOT/instances/default/compute.toml"
DATA="$ROOT/instances/default/data"
MARKER="$ROOT/.compose-initialized"

if [ -f "$MARKER" ]; then
  echo "init-volume: already initialized"
  exit 0
fi
if [ -n "$(find "$ROOT" -mindepth 1 -maxdepth 1 -print -quit)" ]; then
  echo "init-volume: refusing an unmarked nonempty volume; preserve it for operator recovery" >&2
  exit 1
fi
: "${OPEN_COMPUTE_ADMIN_TOKEN:?set OPEN_COMPUTE_ADMIN_TOKEN}"
: "${OPEN_COMPUTE_DEPLOYER_TOKEN:?set OPEN_COMPUTE_DEPLOYER_TOKEN}"
: "${OPEN_COMPUTE_READ_ONLY_TOKEN:?set OPEN_COMPUTE_READ_ONLY_TOKEN}"
if [ "$OPEN_COMPUTE_ADMIN_TOKEN" = "$OPEN_COMPUTE_DEPLOYER_TOKEN" ] ||
   [ "$OPEN_COMPUTE_ADMIN_TOKEN" = "$OPEN_COMPUTE_READ_ONLY_TOKEN" ] ||
   [ "$OPEN_COMPUTE_DEPLOYER_TOKEN" = "$OPEN_COMPUTE_READ_ONLY_TOKEN" ]; then
  echo "init-volume: authentication tokens must be distinct" >&2
  exit 1
fi
case "${OC_STORAGE_BACKEND:-local}" in
  local) ;;
  s3)
    : "${OC_S3_ENDPOINT:?set OC_S3_ENDPOINT}"
    : "${OC_S3_BUCKET:?set OC_S3_BUCKET}"
    : "${OC_S3_ACCESS_KEY_ID:?set OC_S3_ACCESS_KEY_ID}"
    : "${OC_S3_SECRET_ACCESS_KEY:?set OC_S3_SECRET_ACCESS_KEY}"
    ;;
  *) echo "init-volume: unknown storage backend" >&2; exit 1 ;;
esac

# Drop privilege once; every daemon below is a directly owned child.
if [ "$(id -u)" = 0 ]; then
  chmod 700 "$ROOT"
  chown open-compute:open-compute "$ROOT"
  exec runuser -u open-compute -- "$0"
fi

# Encode configuration strings without shell evaluation or a second expansion pass.
toml_string() {
  printf '"'
  printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
  printf '"'
}
as_ocd() {
  "$OCD" --no-update-check --system "$@"
}
mkdir -p "$ROOT/keys"
chmod 700 "$ROOT" "$ROOT/keys"
printf '%s\n' "$OPEN_COMPUTE_ADMIN_TOKEN" > "$ROOT/keys/admin.token"
{
  printf '[server]\npublic_bind = '
  toml_string "${OC_PUBLIC_BIND:-0.0.0.0:8787}"
  printf '\nadmin_auth = { file = "%s/keys/admin.token" }\n' "$ROOT"
} > "$ROOT/ocd.toml"

binary_digest=$(sha256sum "$OCD" | cut -d ' ' -f1)
"$OCD" --no-update-check --system run > "$ROOT/init-daemon.log" 2>&1 &
daemon=$!
daemon_start=$(awk '{print $22}' "/proc/$daemon/stat")
stop_daemon() {
  if kill -0 "$daemon" 2>/dev/null; then
    test "$(awk '{print $22}' "/proc/$daemon/stat")" = "$daemon_start" || return 1
    test "$(sha256sum "/proc/$daemon/exe" | cut -d ' ' -f1)" = "$binary_digest" || return 1
    kill -TERM "$daemon"
  fi
  wait "$daemon"
}
cleanup() {
  stop_daemon || true
}
trap cleanup EXIT
trap 'exit 1' INT TERM
count=0
until as_ocd status --json 2>/dev/null | grep -F '"state":"running"' >/dev/null; do
  if ! kill -0 "$daemon" 2>/dev/null || [ "$count" -ge 120 ]; then
    echo "init-volume: bootstrap daemon did not become ready; inspect init-daemon.log in the volume" >&2
    exit 1
  fi
  count=$((count + 1))
  sleep 1
done
as_ocd instance setup --name default --config "$CONFIG" --data-dir "$DATA" \
  --yes --autostart=true --start=false
stop_daemon
trap - EXIT INT TERM

# Setup created the database, master key and private token files. Configure them
# before the first instance start; never switch object authority on running data.
printf '%s\n' "$OPEN_COMPUTE_DEPLOYER_TOKEN" > "$DATA/keys/deployer.token"
printf '%s\n' "$OPEN_COMPUTE_READ_ONLY_TOKEN" > "$DATA/keys/read-only.token"
{
  cat <<CONFIG
[instance]
name = "default"
[auth]
deployer_auth = { file = "$DATA/keys/deployer.token" }
read_only_auth = { file = "$DATA/keys/read-only.token" }
[data]
path = "$DATA"
master_key_file = "$DATA/keys/master.key"
[dashboard]
enabled = true
[artifacts]
public_origin = "http://127.0.0.1:${OC_PUBLIC_PORT:-8787}"
[observability]
external_control_origin = "http://127.0.0.1:${OC_PUBLIC_PORT:-8787}"
[storage]
CONFIG
  if [ "${OC_STORAGE_BACKEND:-local}" = s3 ]; then
    printf 'backend = "s3"\nforce_path_style = true\nendpoint = '
    toml_string "$OC_S3_ENDPOINT"
    printf '\nregion = '; toml_string "${OC_S3_REGION:-us-east-1}"
    printf '\nbucket = '; toml_string "$OC_S3_BUCKET"
    printf '\nprefix = '; toml_string "${OC_S3_PREFIX:-open-compute/system/}"
    printf '\nr2_prefix = '; toml_string "${OC_S3_R2_PREFIX:-open-compute/tenant/r2/}"
    printf '\naccess_key_id_env = "OC_S3_ACCESS_KEY_ID"\nsecret_access_key_env = "OC_S3_SECRET_ACCESS_KEY"\n'
  else
    printf 'backend = "local"\nprefix = "system/"\nr2_prefix = "tenant/r2/"\n'
  fi
} > "$CONFIG"
as_ocd --config "$CONFIG" config check
# Also validate/start the actual configured instance before declaring bootstrap complete.
"$OCD" --no-update-check --system run >> "$ROOT/init-daemon.log" 2>&1 &
daemon=$!
daemon_start=$(awk '{print $22}' "/proc/$daemon/stat")
trap cleanup EXIT
trap 'exit 1' INT TERM
count=0
until as_ocd instances --json 2>/dev/null | grep -F '"state":"running"' >/dev/null; do
  if as_ocd instances --json 2>/dev/null | grep -F '"state":"failed"' >/dev/null; then
    echo "init-volume: configured instance failed; inspect init-daemon.log in the volume" >&2
    exit 1
  fi
  if ! kill -0 "$daemon" 2>/dev/null || [ "$count" -ge 120 ]; then
    echo "init-volume: configured instance did not start; inspect init-daemon.log in the volume" >&2
    exit 1
  fi
  count=$((count + 1))
  sleep 1
done
stop_daemon
trap - EXIT INT TERM
touch "$MARKER"
echo "init-volume: bootstrap complete"
