#!/bin/sh
# Run inside a disposable distribution container as a non-root user after install.sh:
# start the installed ocd in the foreground, register one instance, deploy a module
# Worker through the Cloudflare API and print its response.
set -eu
ocd="$HOME/.local/bin/ocd"
root="$HOME/.open-compute"
base=http://127.0.0.1:8787
umask 077
mkdir -p "$root/keys"
head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > "$root/keys/admin.token"
printf '[server]\nadmin_auth = { file = "./keys/admin.token" }\n' > "$root/ocd.toml"
token=$(cat "$root/keys/admin.token")

"$ocd" --no-update-check run > "$root/run.log" 2>&1 &
daemon=$!
trap 'kill "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true' EXIT
wait_ready() {
  attempts=0
  until curl -fsS -o /dev/null "$base/health/ready" 2>/dev/null; do
    kill -0 "$daemon" 2>/dev/null || { tail -n 50 "$root/run.log" >&2; exit 1; }
    attempts=$((attempts + 1))
    [ "$attempts" -lt 900 ] || { echo 'daemon readiness timed out' >&2; tail -n 50 "$root/run.log" >&2; exit 1; }
    sleep 0.1
  done
}
wait_ready
"$ocd" --no-update-check instance setup --yes --name smoke >&2
wait_ready

api() {
  curl -fsS -H "Authorization: Bearer $token" -H 'Host: localhost:8787' "$@"
}
account=$(api "$base/client/v4/accounts" | sed -n 's/.*"id":"\([0-9a-f]\{32\}\)".*/\1/p' | head -n 1)
[ -n "$account" ] || { echo 'no account returned' >&2; exit 1; }

work=$(mktemp -d)
printf '{"main_module":"index.js","compatibility_date":"2026-09-08"}' > "$work/metadata.json"
printf 'export default { fetch() { return new Response("distro-smoke-ok"); } };\n' > "$work/index.js"
api -X PUT "$base/client/v4/accounts/$account/workers/scripts/smoke" \
  -F "metadata=@$work/metadata.json;type=application/json" \
  -F "index.js=@$work/index.js;filename=index.js;type=application/javascript+module" > /dev/null
attempts=0
until response=$(curl -fsS -H "Host: smoke.$account.localhost" "$base/" 2>/dev/null) \
  && [ "$response" = distro-smoke-ok ]; do
  attempts=$((attempts + 1))
  [ "$attempts" -lt 300 ] || { echo "Worker response: ${response:-none}" >&2; tail -n 50 "$root/run.log" >&2; exit 1; }
  sleep 0.1
done
printf '%s\n' "$response"
