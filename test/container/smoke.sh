#!/bin/sh
# Exercise release containers on an isolated, disposable Compose project.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"
run_dir=$(mktemp -d "$root/.temp/container-smoke.XXXXXX")
export COMPOSE_PROJECT_NAME="oc-container-$(basename "$run_dir" | tr '[:upper:].' '[:lower:]-')"
export OC_IMAGE=${OC_IMAGE:-open-compute:local}
export OC_INIT_IMAGE=${OC_INIT_IMAGE:-open-compute-init:local}
export OC_PUBLIC_PORT=${OC_PUBLIC_PORT:-18787}
export OPEN_COMPUTE_ADMIN_TOKEN="container-admin-'literal-\$dollar"
export OPEN_COMPUTE_DEPLOYER_TOKEN="container-deployer-'literal-\$dollar"
export OPEN_COMPUTE_READ_ONLY_TOKEN="container-readonly-'literal-\$dollar"
compose() { docker compose -f examples/container/docker-compose.yml "$@"; }
cleanup() {
  result=$?
  compose logs --no-color > "$run_dir/compose.log" 2>&1 || true
  compose ps --all --format json > "$run_dir/containers.json" 2>&1 || true
  # Only this test's fresh project/volume is eligible for teardown.
  if [ "$result" -eq 0 ]; then
    compose down --volumes >/dev/null
  else
    compose stop >/dev/null 2>&1 || true
    echo "container smoke failed; evidence: $run_dir; retained project: $COMPOSE_PROJECT_NAME" >&2
  fi
}
trap cleanup EXIT
trap 'exit 1' INT TERM
compose up -d --pull never --wait --wait-timeout 240
container=$(compose ps -q ocd)
status() {
  compose exec -T ocd /opt/open-compute/ocd --no-update-check --system instances --json
}
wait_instance() {
  count=0
  while :; do
    status > "$1"
    if grep -F '"state":"running"' "$1" >/dev/null; then return; fi
    if grep -F '"state":"failed"' "$1" >/dev/null || [ "$count" -ge 120 ]; then
      echo "instance failed to become ready; inspect $1" >&2
      return 1
    fi
    count=$((count + 1))
    sleep 1
  done
}
wait_instance "$run_dir/before.json"
python3 - "$run_dir/before.json" <<'PY'
import json,sys
value=json.load(open(sys.argv[1]))['instances']
assert len(value)==1 and value[0]['state']=='running', value
PY
# Deliberately quoted token strings must survive bootstrap verbatim.
compose exec -T ocd sh -c 'test "$(stat -c %a /var/lib/open-compute/instances/default/data/keys/master.key)" = 600'
compose exec -T ocd cat /var/lib/open-compute/instances/default/data/keys/deployer.token > "$run_dir/token"
test "$(cat "$run_dir/token")" = "$OPEN_COMPUTE_DEPLOYER_TOKEN"
compose exec -T ocd sha256sum /var/lib/open-compute/instances/default/data/keys/master.key > "$run_dir/master.before"
docker top "$container" -eo pid,args > "$run_dir/argv"
if grep -F -e "$OPEN_COMPUTE_ADMIN_TOKEN" -e "$OPEN_COMPUTE_DEPLOYER_TOKEN" -e "$OPEN_COMPUTE_READ_ONLY_TOKEN" "$run_dir/argv"; then
  echo "credential leaked into process arguments" >&2
  exit 1
fi
compose restart ocd
compose up -d --pull never --wait --wait-timeout 180
wait_instance "$run_dir/after.json"
compose exec -T ocd sha256sum /var/lib/open-compute/instances/default/data/keys/master.key > "$run_dir/master.after"
cmp "$run_dir/master.before" "$run_dir/master.after"
python3 - "$run_dir/before.json" "$run_dir/after.json" <<'PY'
import json,sys
before,after=(json.load(open(p))['instances'][0] for p in sys.argv[1:])
assert after['state']=='running', after
assert before['instance_id']==after['instance_id'], (before,after)
PY
# A later init run must be a no-op and leave the same authority intact.
compose run --rm --no-deps init > "$run_dir/reinit.log"
grep -F 'already initialized' "$run_dir/reinit.log"
compose exec -T ocd sha256sum /var/lib/open-compute/instances/default/data/keys/master.key > "$run_dir/master.reinit"
cmp "$run_dir/master.before" "$run_dir/master.reinit"
# Unavailable S3 must fail closed and preserve an unmarked volume for recovery.
negative_volume="${COMPOSE_PROJECT_NAME}-unavailable-s3"
docker volume create "$negative_volume" >/dev/null
if docker run --rm --mount "type=volume,src=$negative_volume,dst=/var/lib/open-compute" \
  --env OPEN_COMPUTE_ADMIN_TOKEN --env OPEN_COMPUTE_DEPLOYER_TOKEN --env OPEN_COMPUTE_READ_ONLY_TOKEN \
  --env OC_STORAGE_BACKEND=s3 --env OC_S3_ENDPOINT=http://127.0.0.1:9 \
  --env OC_S3_BUCKET=container-test --env OC_S3_ACCESS_KEY_ID=fixture-access \
  --env OC_S3_SECRET_ACCESS_KEY=fixture-secret "$OC_INIT_IMAGE" > "$run_dir/unavailable-s3.log" 2>&1; then
  echo "unavailable S3 unexpectedly initialized; retained volume: $negative_volume" >&2
  exit 1
fi
grep -F 'configured instance failed' "$run_dir/unavailable-s3.log"
docker run --rm --entrypoint sh \
  --mount "type=volume,src=$negative_volume,dst=/var/lib/open-compute,readonly" \
  "$OC_IMAGE" -c 'test ! -e /var/lib/open-compute/.compose-initialized && test -s /var/lib/open-compute/instances/default/data/control.sqlite'
if docker run --rm --mount "type=volume,src=$negative_volume,dst=/var/lib/open-compute" \
  "$OC_INIT_IMAGE" > "$run_dir/interrupted-reinit.log" 2>&1; then
  echo "unmarked nonempty volume unexpectedly accepted; retained volume: $negative_volume" >&2
  exit 1
fi
grep -F 'refusing an unmarked nonempty volume' "$run_dir/interrupted-reinit.log"
docker volume rm "$negative_volume" >/dev/null
echo "container smoke passed: fresh setup, private credentials, restart, repeat init and failed-bootstrap preservation"
