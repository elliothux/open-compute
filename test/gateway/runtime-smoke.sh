#!/bin/sh
set -eu

ocd=$1
data=/tmp/open-compute-gateway-data
config=/tmp/open-compute-gateway.toml
log=/tmp/open-compute-gateway.log

"$ocd" --no-update-check config init --data-dir "$data" >"$config"
cat >>"$config" <<'EOF'

[public_gateway]
base_domain = "gateway-test.open-compute.dev"
ingress_ipv4 = ["1.1.1.1"]
https_listen = "127.0.0.1:18443"
challenge_dns_listen = "127.0.0.1:18053"
EOF

export OPEN_COMPUTE_ADMIN_TOKEN=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
export OPEN_COMPUTE_DEPLOYER_TOKEN=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
export OPEN_COMPUTE_READ_ONLY_TOKEN=cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc
"$ocd" --config "$config" --no-update-check run >"$log" 2>&1 &
parent=$!
trap 'kill -TERM "$parent" 2>/dev/null || true; wait "$parent" 2>/dev/null || true; cat "$log"' EXIT

# Live gateway sockets live under the bounded instance runtime root
# (see instance_control::user_runtime_root + run/execution::prepare_gateway_services),
# not under $data/gateway/run/.
if [ -n "${XDG_RUNTIME_DIR:-}" ]; then
    runtime_scope=${XDG_RUNTIME_DIR%/}/open-compute
else
    runtime_scope=/tmp/open-compute-$(id -u)
fi

admin_sock=
count=0
while [ -z "$admin_sock" ]; do
    for candidate in "$runtime_scope"/*/gateway/admin.sock; do
        if [ -S "$candidate" ]; then
            admin_sock=$candidate
            break
        fi
    done
    if [ -n "$admin_sock" ]; then
        break
    fi
    if ! kill -0 "$parent" 2>/dev/null; then
        exit 1
    fi
    count=$((count + 1))
    [ "$count" -lt 120 ] || exit 1
    sleep 1
done

curl --fail --silent --unix-socket "$admin_sock" http://localhost/config/ >/dev/null
pgrep -f 'run --config .*/gateway/Caddyfile --adapter caddyfile' >/dev/null
"$ocd" --config "$config" --no-update-check caddy status | grep -F 'CADDY_STATUS child_pid='
"$ocd" --config "$config" --no-update-check caddy validate | grep -Fx 'CADDY_CONFIG_OK'
"$ocd" --config "$config" --no-update-check caddy reload | grep -F 'CADDY_RELOAD_OK child_pid='
test -s "$data/gateway/config-state/current.json"
test -s "$data/gateway/config-state/current.meta.json"
test "$(stat -c '%a' "$data/gateway/config-state/current.json")" = 600
kill -TERM "$parent"
wait "$parent"
! pgrep -f 'run --config .*/gateway/Caddyfile --adapter caddyfile' >/dev/null
! pgrep -x workerd >/dev/null

# A restart uses the last confirmed same-pin/same-intent JSON snapshot even if
# operator source files have not been reparsed.
"$ocd" --config "$config" --no-update-check run >>"$log" 2>&1 &
parent=$!
count=0
while ! pgrep -f 'run --config .*/gateway/config-state/current.json' >/dev/null; do
    if ! kill -0 "$parent" 2>/dev/null; then
        exit 1
    fi
    count=$((count + 1))
    [ "$count" -lt 120 ] || exit 1
    sleep 1
done
"$ocd" --config "$config" --no-update-check caddy status | grep -F 'config_sha256='
kill -TERM "$parent"
wait "$parent"
trap - EXIT
! pgrep -f 'run --config .*/gateway/config-state/current.json' >/dev/null
! pgrep -x workerd >/dev/null
