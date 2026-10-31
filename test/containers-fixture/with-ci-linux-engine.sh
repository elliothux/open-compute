#!/usr/bin/env bash
# Own a quota-backed Linux CI Engine around one canonical Broker command.
set -euo pipefail

run_linux_engine() {
images= binary= root= evidence=
while (($#)); do
  case "$1" in
    --images) images=$2; shift 2 ;;
    --binary) binary=$2; shift 2 ;;
    --root) root=$2; shift 2 ;;
    --evidence) evidence=$2; shift 2 ;;
    --command) shift; break ;;
    *) echo 'Unknown Engine argument' >&2; exit 1 ;;
  esac
done
test "$(uname -s)" = Linux
test "$(id -u)" != 0
test ! -e .env
test -n "$images" && test -n "$binary" && (($# > 0))
python3 - "$root" "$evidence" <<'PY'
import os
import json
from pathlib import Path
import shutil
import stat
import subprocess
import sys
root, evidence = map(Path, sys.argv[1:])
assert root.is_absolute() and root.parent == Path('/tmp') and root.name.startswith('oc-ci-')
assert len(os.fsencode(root)) <= 48 and not os.path.lexists(root)
assert evidence.is_absolute() and evidence.parent.resolve() == evidence.parent
assert evidence.is_relative_to(Path.cwd() / '.temp') and not os.path.lexists(evidence)
disk = shutil.disk_usage(root.parent)
required = (44 << 30) + disk.total // 20 + 1
admission = {'initialFreeBytes': disk.free, 'totalBytes': disk.total,
             'requiredFreeBytes': required, 'removedPreinstalledSdks': [],
             'rejectedPreinstalledSdks': []}
evidence.mkdir(parents=True)
def record():
    admission['finalFreeBytes'] = shutil.disk_usage(root.parent).free
    (evidence / 'disk-admission.json').write_text(json.dumps(admission) + '\n')
record()
# Hosted images carry unrelated SDKs; reclaim only idle, root-owned SDK directories.
if (disk.free < required and os.environ.get('GITHUB_ACTIONS') == 'true'
        and os.environ.get('RUNNER_ENVIRONMENT') == 'github-hosted'):
    for name in ('/usr/local/lib/android', '/usr/share/dotnet', '/usr/local/.ghcup'):
        path = Path(name)
        if not path.exists():
            continue
        info = path.lstat()
        if not (stat.S_ISDIR(info.st_mode) and info.st_uid == 0 and path.resolve() == path):
            admission['rejectedPreinstalledSdks'].append({'path': name, 'reason': 'identity'})
            record()
            continue
        before = shutil.disk_usage(root.parent).free
        # The privileged child checks live references and mount boundaries before removal.
        cleanup = subprocess.run(['sudo', '-n', 'python3', '-I', '-B', '-', name,
                        str(info.st_dev), str(info.st_ino)], capture_output=True, input=r'''
import os, shutil, stat, sys
from pathlib import Path
path = Path(sys.argv[1])
assert str(path) in ('/usr/local/lib/android', '/usr/share/dotnet', '/usr/local/.ghcup')
info = path.lstat()
assert (info.st_dev, info.st_ino) == tuple(map(int, sys.argv[2:]))
assert stat.S_ISDIR(info.st_mode) and info.st_uid == 0 and path.resolve() == path
prefix = str(path) + '/'
def inside(value):
    return value == str(path) or value.startswith(prefix)
for line in Path('/proc/self/mountinfo').read_text().splitlines():
    assert not inside(line.split()[4])
for process in Path('/proc').glob('[0-9]*'):
    try:
        for link in [process / 'exe', process / 'cwd', *list((process / 'fd').iterdir())]:
            try:
                assert not inside(os.readlink(link).removesuffix(' (deleted)'))
            except FileNotFoundError:
                pass
        for line in (process / 'maps').read_text().splitlines():
            fields = line.split(maxsplit=5)
            assert len(fields) < 6 or not inside(fields[5].removesuffix(' (deleted)'))
    except (FileNotFoundError, ProcessLookupError):
        pass
assert (path.lstat().st_dev, path.lstat().st_ino) == (info.st_dev, info.st_ino)
shutil.rmtree(path)
''', text=True)
        if cleanup.returncode:
            admission['rejectedPreinstalledSdks'].append({
                'path': name, 'reason': 'inspection-or-removal', 'exitCode': cleanup.returncode})
            record()
            continue
        admission['removedPreinstalledSdks'].append({
            'path': name, 'freeBytesBefore': before,
            'freeBytesAfter': shutil.disk_usage(root.parent).free})
        record()
        if admission['finalFreeBytes'] >= required:
            break
record()
assert admission['finalFreeBytes'] >= required, 'CI_ENGINE_DISK_ADMISSION_FAILED'
PY
for tool in docker dockerd sudo systemd-run systemctl timeout mount umount findmnt tee; do
  command -v "$tool" >/dev/null
done
# Refuse a runner with live or stopped workloads before stopping its default daemon.
default_containers=$(timeout 10 docker --host unix:///var/run/docker.sock container ls -aq)
test -z "$default_containers"
default_engine=$(timeout 10 docker --host unix:///var/run/docker.sock info --format '{{.ID}}')
test -n "$default_engine"
default_images=$(timeout 10 docker --host unix:///var/run/docker.sock image ls -aq | sort -u)
default_volumes=$(timeout 10 docker --host unix:///var/run/docker.sock volume ls -q | sort)
network_snapshot() {
  python3 - <<'PY'
import json
import re
import subprocess
docker = ['docker', '--host', 'unix:///var/run/docker.sock', 'network']
ids = subprocess.check_output([*docker, 'ls', '-q', '--no-trunc'], text=True, timeout=10).split()
assert ids and all(re.fullmatch('[0-9a-f]{64}', value) for value in ids)
rows = json.loads(subprocess.check_output([*docker, 'inspect', *ids], text=True, timeout=10))
assert isinstance(rows, list) and sorted(row['Id'] for row in rows) == sorted(ids)
for row in rows:
    # dockerd recreates its builtin bridge; preserve every configuration field.
    if (row['Name'] == 'bridge' and row['Driver'] == 'bridge' and row['Scope'] == 'local'
            and row.get('Options', {}).get('com.docker.network.bridge.default_bridge') == 'true'):
        del row['Id']
        row.pop('Created', None)
print(json.dumps(sorted(rows, key=lambda row: row['Name']), sort_keys=True))
PY
}
default_networks=$(network_snapshot)
sudo -n true
umask 077
mkdir "$root"
mkdir -p "$evidence"
unit="oc-ci-engine-$(id -u)-$$"
slice="occi$(id -u)p$$.slice"
default_stopped=0
mounted=0
engine_started=0
slice_configured=0
cleanup() {
  status=$?
  trap - EXIT
  set +e
  cleanup_status=0
  # Retain the original command evidence before teardown can interrupt cleanup.
  printf 'CI Engine cleanup: command_exit=%s phase=preserve-evidence\n' "$status" >&2
  for path in result.json engine-events.jsonl health-observations.jsonl helper-result.json \
      broker/evidence/result.json broker/evidence/command.log \
      broker/evidence/broker.stdout broker/evidence/broker.stderr; do
    if test -f "$root/fixture/$path"; then
      mkdir -p "$evidence/$(dirname "$path")"
      cp "$root/fixture/$path" "$evidence/$path" || cleanup_status=1
    fi
  done
  engine_alive=0
  if ((engine_started)); then
    timeout 180 sudo -n systemctl stop "$unit" || cleanup_status=1
    if sudo -n systemctl is-active --quiet "$unit"; then
      cleanup_status=1
      sudo -n systemctl kill --kill-whom=all --signal=KILL "$unit"
      timeout 30 sudo -n systemctl stop "$unit"
      if sudo -n systemctl is-active --quiet "$unit"; then engine_alive=1; fi
    fi
    if ((engine_alive == 0)); then
      # Reap any residual container descendants in this run's resource slice.
      if ! printf '1\n' | sudo -n tee "/sys/fs/cgroup/$slice/cgroup.kill" >/dev/null; then
        cleanup_status=1
        engine_alive=1
      fi
      if ! timeout 30 sudo -n systemctl stop "$slice"; then
        cleanup_status=1
        engine_alive=1
      fi
      if ((engine_alive == 0)); then
        timeout 15 sudo -n systemctl revert "$slice" || cleanup_status=1
      fi
    fi
  elif ((slice_configured)); then
    if timeout 30 sudo -n systemctl stop "$slice"; then
      timeout 15 sudo -n systemctl revert "$slice" || cleanup_status=1
    else
      cleanup_status=1
    fi
  fi
  # An Engine that did not stop must retain its backing filesystem.
  if ((mounted && engine_alive == 0)); then
    sudo -n umount "$root/data" || cleanup_status=1
    if findmnt -n --mountpoint "$root/data" >/dev/null; then cleanup_status=1; fi
  fi
  if ((default_stopped && engine_alive == 0)); then
    timeout 60 sudo -n systemctl start docker.socket docker.service || cleanup_status=1
    restored=$(timeout 10 docker --host unix:///var/run/docker.sock info --format '{{.ID}}') || cleanup_status=1
    images_after=$(timeout 10 docker --host unix:///var/run/docker.sock image ls -aq | sort -u) || cleanup_status=1
    volumes_after=$(timeout 10 docker --host unix:///var/run/docker.sock volume ls -q | sort) || cleanup_status=1
    networks_after=$(network_snapshot) || cleanup_status=1
    test "$images_after" = "$default_images" || cleanup_status=1
    test "$volumes_after" = "$default_volumes" || cleanup_status=1
    test "$networks_after" = "$default_networks" || cleanup_status=1
    test "$restored" = "$default_engine" || cleanup_status=1
    containers_after=$(timeout 10 docker --host unix:///var/run/docker.sock container ls -aq) || cleanup_status=1
    test -z "$containers_after" || cleanup_status=1
  fi
  python3 - "$evidence" "$status" "$cleanup_status" <<'PY' || cleanup_status=1
import json
from pathlib import Path
import sys
path, command, cleanup = sys.argv[1:]
(Path(path) / 'engine-result.json').write_text(json.dumps({
    'success': command == '0' and cleanup == '0',
    'commandExitCode': int(command), 'cleanupExitCode': int(cleanup),
    'fullWorkspaceQualifiedByWrapper': False}) + '\n')
PY
  if ((status == 0)); then status=$cleanup_status; fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
sudo -n apt-get update -qq > "$evidence/tools.log" 2>&1
sudo -n apt-get install -y --no-install-recommends xfsprogs >> "$evidence/tools.log" 2>&1
truncate -s 12G "$root/xfs.img"
mkfs.xfs -f "$root/xfs.img" > "$evidence/xfs.log" 2>&1
mkdir "$root/data"
sudo -n mount -o loop,pquota "$root/xfs.img" "$root/data"
mounted=1
findmnt -n -o FSTYPE,OPTIONS --mountpoint "$root/data" | grep -E '^xfs .*prjquota' >> "$evidence/xfs.log"
printf '%s\n' '{"features":{"containerd-snapshotter":false}}' > "$root/daemon.json"
# Apply shared limits before either the daemon or container cgroups can start.
slice_configured=1
sudo -n systemctl set-property --runtime "$slice" CPUQuota=200% MemoryMax=4G
default_stopped=1
timeout 60 sudo -n systemctl stop docker.socket docker.service
engine_started=1
sudo -n systemd-run --unit "$unit" --slice "$slice" --collect --service-type=exec --property=Delegate=yes \
  dockerd --config-file "$root/daemon.json" --data-root "$root/data/docker" \
  --exec-root "$root/exec" --pidfile "$root/docker.pid" --host "unix://$root/docker.sock" \
  --storage-driver overlay2 --exec-opt native.cgroupdriver=cgroupfs \
  --cgroup-parent "/$slice" > "$evidence/start.log" 2>&1
for ((attempt=0; attempt<120; attempt++)); do
  if timeout 10 docker --host "unix://$root/docker.sock" info >/dev/null 2>&1; then break; fi
  sleep 0.5
done
timeout 10 docker --host "unix://$root/docker.sock" info >/dev/null
python3 -I -B test/containers-fixture/with-ci-broker.py \
  --images "$images" --engine-socket "$root/docker.sock" \
  --binary "$binary" --root "$root/fixture" --command "$@" 2>&1 | tee "$evidence/pipeline.log"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  # This entry owns the disposable Actions runner, never a developer's Engine.
  test "${GITHUB_ACTIONS:-}" = true
  run_linux_engine "$@"
fi
