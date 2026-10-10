#!/usr/bin/env bash
# Qualify the packaged Linux x64 candidate in a unique, resource-limited Compose project.
set -euo pipefail
umask 077
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"
candidate=${1:?usage: qualify-release.sh ABS_CANDIDATE ABS_PACKAGE_REPORT}
package_report=${2:?usage: qualify-release.sh ABS_CANDIDATE ABS_PACKAGE_REPORT}
[ "$#" -eq 2 ]
mkdir -p .temp/release-stress
export STRESS_RUN_ROOT
STRESS_RUN_ROOT=$(mktemp -d "$root/.temp/release-stress/run.XXXXXX")
export OPEN_COMPUTE_ROOT="$root" STRESS_ACCOUNT_ID=bootstrap
export COMPOSE_PROJECT_NAME="oc-stress-$(basename "$STRESS_RUN_ROOT" | tr '[:upper:].' '[:lower:]-')"
export OC_IMAGE="open-compute-stress:${COMPOSE_PROJECT_NAME}" OC_INIT_IMAGE="open-compute-stress-init:${COMPOSE_PROJECT_NAME}"
# Match the container listener so strict control-plane Host validation succeeds.
export OC_PUBLIC_PORT=8787 OC_HOST_BIND=127.0.0.1 OC_STORAGE_BACKEND=local
export STRESS_BASE_URL="http://127.0.0.1:${OC_PUBLIC_PORT}"
export STRESS_PROFILE=2c4g STRESS_P1_ABBREV=0 STRESS_P1_SCALE=1 STRESS_SOAK_ABBREV=0 STRESS_SCENARIO_MODE=normal
unset STRESS_P1_STACK
export OPEN_COMPUTE_ADMIN_TOKEN="${COMPOSE_PROJECT_NAME}-admin"
export OPEN_COMPUTE_DEPLOYER_TOKEN="${COMPOSE_PROJECT_NAME}-deployer"
export OPEN_COMPUTE_READ_ONLY_TOKEN="${COMPOSE_PROJECT_NAME}-readonly"
export STRESS_COMPOSE_OVERRIDE="$STRESS_RUN_ROOT/resources.yml"
printf 'services:\n  ocd:\n    cpus: 2\n    mem_limit: 4g\n' > "$STRESS_COMPOSE_OVERRIDE"
. "$root/test/stress/common.sh"

# Verify both the packaged bytes and the executable's embedded source/runtime identity.
python3 - "$candidate" "$package_report" <<'PY'
import hashlib,json,subprocess,sys
from pathlib import Path
binary,report=Path(sys.argv[1]),json.loads(Path(sys.argv[2]).read_text())
assert binary.is_absolute() and report['schemaVersion']==1 and report['target']=='linux-x64'
assert report['revision']==subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()
assert report['bytes']==binary.stat().st_size and report['sha256']==hashlib.sha256(binary.read_bytes()).hexdigest()
assert report['workerdLockSha256']==hashlib.sha256(Path('packages/runtime/workerd.lock.json').read_bytes()).hexdigest()
assert subprocess.check_output([str(binary),'--version'],text=True).strip()=='ocd '+report['version']
PY
bun scripts/verify-release-executable.ts "$candidate" "$(git rev-parse HEAD)"
# A fresh checkout must not replace an operator's existing container build input.
[ ! -e examples/container/ocd.linux-amd64 ]
cp "$candidate" examples/container/ocd.linux-amd64
cleanup() {
  status=$?
  stress_compose logs --no-color > "$STRESS_RUN_ROOT/compose.log" 2>&1 || true
  if [ "$status" -eq 0 ]; then
    stress_compose down --volumes >/dev/null
  else
    stress_compose stop >/dev/null 2>&1 || true
    printf 'stress qualification failed; evidence retained at %s, project %s\n' "$STRESS_RUN_ROOT" "$COMPOSE_PROJECT_NAME" >&2
  fi
  rm examples/container/ocd.linux-amd64
}
trap cleanup EXIT
trap 'exit 1' INT TERM
docker build --target init -t "$OC_INIT_IMAGE" examples/container > "$STRESS_RUN_ROOT/build-init.log" 2>&1
docker build --target runtime -t "$OC_IMAGE" examples/container > "$STRESS_RUN_ROOT/build-runtime.log" 2>&1
stress_compose up -d --pull never --wait --wait-timeout 240
container=$(stress_container)
# Record actual Docker-enforced limits, not the host's advertised resource profile.
docker inspect --format '{{json .HostConfig}}' "$container" > "$STRESS_RUN_ROOT/resources.json"
python3 - "$STRESS_RUN_ROOT/resources.json" <<'PY'
import json,sys
config=json.load(open(sys.argv[1]))
assert config['NanoCpus']==2_000_000_000 and config['Memory']==4*1024**3
PY
bash test/stress/deploy-stress-demo.sh > "$STRESS_RUN_ROOT/deploy.log" 2>&1
. "$STRESS_RUN_ROOT/.deploy_env"
for script in stress-smoke stress-p0 stress-scenario stress-p1 stress-p1-soak; do
  start=$(date +%s)
  bash "test/stress/${script}.sh" > "$STRESS_RUN_ROOT/${script}.log" 2>&1
  # Each script must emit exactly one new qualification result.
  python3 - "$STRESS_RUN_ROOT" "$script" "$start" <<'PY'
import json,sys,time
from pathlib import Path
root,script,start=Path(sys.argv[1]),sys.argv[2],int(sys.argv[3])
profiles={'stress-smoke':'smoke','stress-p0':'p0-2c4g','stress-scenario':'scenario','stress-p1':'p1-2c4g-peak','stress-p1-soak':'p1-2c4g-soak'}
found=[(p,json.loads(p.read_text())) for p in root.glob('*/result.json') if json.loads(p.read_text()).get('profile')==profiles[script]]
assert len(found)==1 and found[0][1]['verdict']=='pass'
report=found[0][1];report['seconds']=time.time()-start
found[0][0].write_text(json.dumps(report,indent=2)+'\n')
PY
done
python3 - "$STRESS_RUN_ROOT" "$package_report" <<'PY'
import json,sys
from pathlib import Path
root,package=Path(sys.argv[1]),json.loads(Path(sys.argv[2]).read_text())
profiles=['smoke','p0-2c4g','scenario','p1-2c4g-peak','p1-2c4g-soak']
results=[json.loads(p.read_text()) for p in sorted(root.glob('*/result.json'))]
# Reconcile results are internal soak evidence; verified recovery stays in soak.events.
runs=[r for r in results if r.get('profile') in profiles]
assert len(runs)==len(profiles) and {r['profile'] for r in runs}==set(profiles)
assert all(r['verdict']=='pass' for r in runs)
report={'schemaVersion':1,'status':'passed','version':package['version'],'revision':package['revision'],
        'candidateSha256':package['sha256'],'workerdLockSha256':package['workerdLockSha256'],
        'cpuLimit':2,'memoryLimitBytes':4*1024**3,'runs':runs}
(root/'qualification.json').write_text(json.dumps(report,indent=2)+'\n')
PY
printf 'release stress qualification passed: %s\n' "$STRESS_RUN_ROOT/qualification.json"
