#!/usr/bin/env bash
# Own a quota-backed Linux CI Engine around one canonical Broker command.
set -euo pipefail

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
# This entry owns the disposable Actions runner, never a developer's Engine.
test "${GITHUB_ACTIONS:-}:$(uname -s)" = true:Linux
test "$(id -u)" != 0
test ! -e .env
test -n "$images" && test -n "$binary" && (($# > 0))
python3 - "$root" "$evidence" <<'PY'
import os
from pathlib import Path
import shutil
import sys
root, evidence = map(Path, sys.argv[1:])
assert root.is_absolute() and root.parent == Path('/tmp') and root.name.startswith('oc-ci-')
assert len(os.fsencode(root)) <= 48 and not os.path.lexists(root)
assert evidence.is_absolute() and evidence.parent.resolve() == evidence.parent
assert evidence.is_relative_to(Path.cwd() / '.temp') and not os.path.lexists(evidence)
assert shutil.disk_usage(root.parent).free >= 44 << 30
PY
for tool in docker dockerd sudo systemd-run systemctl timeout mount umount findmnt; do
  command -v "$tool" >/dev/null
done
# Refuse a runner with live or stopped workloads before stopping its default daemon.
default_containers=$(timeout 10 docker --host unix:///var/run/docker.sock container ls -aq)
test -z "$default_containers"
default_engine=$(timeout 10 docker --host unix:///var/run/docker.sock info --format '{{.ID}}')
test -n "$default_engine"
default_images=$(timeout 10 docker --host unix:///var/run/docker.sock image ls -aq | sort -u)
default_volumes=$(timeout 10 docker --host unix:///var/run/docker.sock volume ls -q | sort)
default_networks=$(timeout 10 docker --host unix:///var/run/docker.sock network ls -q | sort)
sudo -n true
umask 077
mkdir "$root"
mkdir -p "$evidence"
unit="oc-ci-engine-$(id -u)-$$"
slice="occi$(id -u)p$$.slice"
default_stopped=0
mounted=0
engine_started=0
cleanup() {
  status=$?
  trap - EXIT
  set +e
  cleanup_status=0
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
    networks_after=$(timeout 10 docker --host unix:///var/run/docker.sock network ls -q | sort) || cleanup_status=1
    test "$images_after" = "$default_images" || cleanup_status=1
    test "$volumes_after" = "$default_volumes" || cleanup_status=1
    test "$networks_after" = "$default_networks" || cleanup_status=1
    test "$restored" = "$default_engine" || cleanup_status=1
    containers_after=$(timeout 10 docker --host unix:///var/run/docker.sock container ls -aq) || cleanup_status=1
    test -z "$containers_after" || cleanup_status=1
  fi
  for path in result.json engine-events.jsonl health-observations.jsonl helper-result.json \
      broker/evidence/result.json broker/evidence/command.log \
      broker/evidence/broker.stdout broker/evidence/broker.stderr; do
    if test -f "$root/fixture/$path"; then
      mkdir -p "$evidence/$(dirname "$path")"
      cp "$root/fixture/$path" "$evidence/$path" || cleanup_status=1
    fi
  done
  python3 - "$evidence" "$status" "$cleanup_status" <<'PY'
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
default_stopped=1
timeout 60 sudo -n systemctl stop docker.socket docker.service
engine_started=1
sudo -n systemd-run --unit "$unit" --slice "$slice" --collect --service-type=exec --property=Delegate=yes \
  dockerd --config-file "$root/daemon.json" --data-root "$root/data/docker" \
  --exec-root "$root/exec" --pidfile "$root/docker.pid" --host "unix://$root/docker.sock" \
  --storage-driver overlay2 --exec-opt native.cgroupdriver=cgroupfs \
  --cgroup-parent "/$slice" > "$evidence/start.log" 2>&1
# The slice contains both dockerd and its container cgroups.
sudo -n systemctl set-property --runtime "$slice" CPUQuota=200% MemoryMax=4G
for ((attempt=0; attempt<120; attempt++)); do
  if timeout 10 docker --host "unix://$root/docker.sock" info >/dev/null 2>&1; then break; fi
  sleep 0.5
done
timeout 10 docker --host "unix://$root/docker.sock" info >/dev/null
python3 -I -B test/containers-fixture/with-ci-broker.py \
  --images "$images" --engine-socket "$root/docker.sock" \
  --binary "$binary" --root "$root/fixture" --command "$@" > "$evidence/pipeline.log" 2>&1
