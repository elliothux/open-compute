#!/usr/bin/env bash
# Bootstrap resources in the selected local Compose project; never patch tracked configuration.
set -euo pipefail
umask 077
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
export OPEN_COMPUTE_ROOT="$root"
export STRESS_ACCOUNT_ID=${STRESS_ACCOUNT_ID:-bootstrap}
. "${root}/test/stress/common.sh"
mkdir -p "$stress_run_root"
deploy_env="${stress_run_root}/.deploy_env"
export CLOUDFLARE_API_BASE_URL="${base_url}/client/v4"
export CF_SEND_TELEMETRY=false DO_NOT_TRACK=1
cf() { node "${root}/node_modules/cf/bin/cf" "$@"; }

# Select the running service, rather than assuming a global container or volume name.
export CLOUDFLARE_API_TOKEN
CLOUDFLARE_API_TOKEN=$(stress_compose exec -T ocd cat /var/lib/open-compute/instances/default/data/keys/deployer.token)
api() { curl -fsS -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" "${CLOUDFLARE_API_BASE_URL}$1"; }
account_id=''
for attempt in $(seq 1 120); do
  if account_id=$(curl -fsS -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" "${CLOUDFLARE_API_BASE_URL}/accounts" | jq -er '.result | if length == 1 then .[0].id else error("expected one stress account") end'); then
    if api "/accounts/${account_id}" | jq -e --arg id "$account_id" '.success == true and .result.id == $id' >/dev/null; then break; fi
  fi
  account_id=''
  sleep 1
done
[ -n "$account_id" ]
export CLOUDFLARE_ACCOUNT_ID="$account_id" STRESS_ACCOUNT_ID="$account_id"

cd "$root"
kv_id=$(cf kv namespaces list | jq -r '[.[] | select(.title == "stress-demo-kv")][0].id // empty')
if [ -z "$kv_id" ]; then
  cf kv namespaces create --title stress-demo-kv >/dev/null
  kv_id=$(cf kv namespaces list | jq -er '[.[] | select(.title == "stress-demo-kv")] | if length == 1 then .[0].id else error("KV inventory") end')
fi
db_id=$(cf d1 list | jq -r '[.[] | select(.name == "stress-demo-db")][0].uuid // empty')
if [ -z "$db_id" ]; then
  cf d1 create --name stress-demo-db >/dev/null
  db_id=$(cf d1 list | jq -er '[.[] | select(.name == "stress-demo-db")] | if length == 1 then .[0].uuid else error("D1 inventory") end')
fi
buckets=$(cf r2 buckets list)
if ! printf '%s' "$buckets" | jq -e '.buckets[] | select(.name == "stress-demo-bucket")' >/dev/null; then
  cf r2 buckets create --name stress-demo-bucket >/dev/null
fi
queue_id=$(cf queues list | jq -r '[.[] | select(.queue_name == "stress-demo-events")][0].queue_id // empty')
if [ -z "$queue_id" ]; then
  cf queues create --queue-name stress-demo-events >/dev/null
  queue_id=$(cf queues list | jq -er '[.[] | select(.queue_name == "stress-demo-events")] | if length == 1 then .[0].queue_id else error("Queue inventory") end')
fi
export STRESS_KV_NAMESPACE_ID="$kv_id" STRESS_D1_DATABASE_ID="$db_id"
# Outbound runs inside the container; its loopback listener is always 8787.
export STRESS_OUTBOUND_URL=${STRESS_OUTBOUND_URL:-http://127.0.0.1:8787/health/live}
cd "${root}/examples/stress-demo"
worker_exists=$(api "/accounts/${account_id}/workers/scripts" | jq -r 'any(.result[]; .id == "stress-demo")')
deploy() { bun run build; bun run deploy:local; }
if [ "$worker_exists" = false ]; then STRESS_INCLUDE_SERVICE=false deploy; fi
STRESS_INCLUDE_SERVICE=true deploy
consumers=$(cf queues consumers list --queue-id "$queue_id")
if ! printf '%s' "$consumers" | jq -e '.[] | select(.script_name == "stress-demo")' >/dev/null; then
  cf queues consumers create "$queue_id" --script-name stress-demo --type worker --settings-batch-size 100 >/dev/null
fi
consumers=$(cf queues consumers list --queue-id "$queue_id")
if ! printf '%s' "$consumers" | jq -e 'length == 1 and .[0].script_name == "stress-demo" and .[0].type == "worker" and .[0].settings.batch_size == 100' >/dev/null; then
  echo "stress consumer requires one stress-demo Worker with batch size 100" >&2
  exit 1
fi
sh "${root}/test/stress/generate-data.sh"
# Quote each value as shell data and create the private file before writing credentials.
: > "$deploy_env"
chmod 600 "$deploy_env"
for key in CLOUDFLARE_API_BASE_URL CLOUDFLARE_ACCOUNT_ID CLOUDFLARE_API_TOKEN STRESS_ACCOUNT_ID STRESS_KV_NAMESPACE_ID STRESS_D1_DATABASE_ID STRESS_OUTBOUND_URL; do
  printf 'export %s=%q\n' "$key" "${!key}" >> "$deploy_env"
done
printf 'export STRESS_BASE_URL=%q\n' "$base_url" >> "$deploy_env"
printf 'stress-demo deployed to selected Compose project %s\n' "${COMPOSE_PROJECT_NAME:-default}"
