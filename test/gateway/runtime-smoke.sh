#!/bin/sh
set -eu

ocd=$1
root=$HOME/.open-compute
config=$root/instances/smoke/compute.toml
data=$root/instances/smoke/data
log=$root/daemon.log
mkdir -p "$root/keys"
chmod 700 "$root" "$root/keys"
printf '%s\n' aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa > "$root/keys/admin.token"
chmod 600 "$root/keys/admin.token"
cat > "$root/ocd.toml" <<EOF
[server]
public_bind = "127.0.0.1:18787"
admin_auth = { file = "$root/keys/admin.token" }

[gateway]
ingress_ipv4 = ["1.1.1.1"]
https_listen = "127.0.0.1:18443"
challenge_dns_listen = "127.0.0.1:18053"
EOF
chmod 600 "$root/ocd.toml"

"$ocd" --no-update-check run > "$log" 2>&1 &
daemon=$!
trap 'kill -TERM "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true; cat "$log"' EXIT
count=0
until "$ocd" --no-update-check status --json 2>/dev/null | grep -F '"state":"running"' >/dev/null; do
    kill -0 "$daemon"
    count=$((count + 1))
    [ "$count" -lt 120 ]
    sleep 1
done
admin_sock=$root/run/gateway/admin.sock
count=0
until curl --fail --silent --unix-socket "$admin_sock" http://localhost/config/ >/dev/null; do
    kill -0 "$daemon"
    count=$((count + 1))
    [ "$count" -lt 120 ]
    sleep 1
done

"$ocd" --no-update-check instance setup --name smoke --config "$config" --data-dir "$data" --autostart=false --start=false --yes
"$ocd" --no-update-check instance remove smoke
printf '\n[public_gateway]\nbase_domain = "gateway-test.open-compute.dev"\n' >> "$config"
"$ocd" --no-update-check instance add --config "$config"

count=0
until curl --fail --silent --unix-socket "$admin_sock" http://localhost/config/ >/dev/null; do
    kill -0 "$daemon"
    count=$((count + 1))
    [ "$count" -lt 120 ]
    sleep 1
done
"$ocd" --no-update-check caddy status | grep -F 'CADDY_STATUS child_pid='
"$ocd" --no-update-check caddy validate | grep -Fx 'CADDY_CONFIG_OK'
"$ocd" --no-update-check caddy reload | grep -F 'CADDY_RELOAD_OK child_pid='
test -s "$root/gateway/config-state/current.json"
test -s "$root/gateway/config-state/current.meta.json"
test "$(stat -c '%a' "$root/gateway/config-state/current.json")" = 600
test ! -e "$data/gateway"

kill -TERM "$daemon"
wait "$daemon"
"$ocd" --no-update-check run >> "$log" 2>&1 &
daemon=$!
count=0
until "$ocd" --no-update-check caddy status | grep -F 'config_sha256=' >/dev/null; do
    kill -0 "$daemon"
    count=$((count + 1))
    [ "$count" -lt 120 ]
    sleep 1
done
kill -TERM "$daemon"
wait "$daemon"
trap - EXIT
! pgrep -x workerd >/dev/null
