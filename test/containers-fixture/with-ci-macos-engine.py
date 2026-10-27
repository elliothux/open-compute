#!/usr/bin/env python3
"""Own an isolated macOS CI Engine around the canonical Broker and one command."""
import argparse
import ctypes
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import signal
import shutil
import socket as unix_sockets
import stat
import subprocess
import sys

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('ci_broker', HERE / 'with-ci-broker.py')
pipeline = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pipeline)
require = pipeline.require

# The backing file lives on Colima's data disk, never on its smaller OS disk.
GUEST_SETUP = r'''set -eu
sudo systemctl stop docker.socket docker.service
sudo apt-get update -qq
sudo apt-get install -y --no-install-recommends xfsprogs
test "$(findmnt -n -o SOURCE --target /var/lib/docker)" != "$(findmnt -n -o SOURCE --target /)"
test "$(df -B1 --output=size /var/lib/docker | tail -1)" -le 17179869184
test "$(df -B1 --output=size / | tail -1)" -le 8589934592
test "$(df -B1 --output=avail /var/lib/docker | tail -1)" -ge 13958643712
test ! -e /var/lib/docker/oc-ci-xfs.img
sudo truncate -s 12G /var/lib/docker/oc-ci-xfs.img
sudo chmod 600 /var/lib/docker/oc-ci-xfs.img
sudo mkfs.xfs -f /var/lib/docker/oc-ci-xfs.img
sudo mkdir -m 700 /mnt/oc-ci-docker
sudo mount -o loop,pquota /var/lib/docker/oc-ci-xfs.img /mnt/oc-ci-docker
findmnt -n -o FSTYPE,OPTIONS --mountpoint /mnt/oc-ci-docker | grep -E '^xfs .*prjquota'
sudo tee /etc/docker/daemon.json >/dev/null <<'JSON'
{"data-root":"/mnt/oc-ci-docker/data","storage-driver":"overlay2","exec-opts":["native.cgroupdriver=cgroupfs"],"features":{"containerd-snapshotter":false}}
JSON
sudo systemctl start docker.socket docker.service
test "$(nproc)" -eq 2
test "$(awk '/MemTotal/ {print $2}' /proc/meminfo)" -ge 3800000
test "$(awk '/MemTotal/ {print $2}' /proc/meminfo)" -le 4194304
test "$(df -B1 --output=avail /mnt/oc-ci-docker | tail -1)" -ge 10737418240
nproc
grep '^MemTotal:' /proc/meminfo
df -B1 / /var/lib/docker /mnt/oc-ci-docker
'''


def agent_forwarding(home):
    path = home / '_lima/colima-fixture/ha.stderr.log'
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    except FileNotFoundError:
        return {'available': False}
    with os.fdopen(fd, 'rb') as stream:
        metadata = os.fstat(stream.fileno())
        require(stat.S_ISREG(metadata.st_mode) and metadata.st_uid == os.getuid(),
                'OWNED_AGENT_LOG_REQUIRED')
        stream.seek(max(0, metadata.st_size - (256 << 10)))
        lines = stream.read(256 << 10).decode('utf8', errors='replace').lower().splitlines()
    return {'available': True, 'truncated': metadata.st_size > 256 << 10,
            'sshVsockFallback': sum('ssh server does not seem to be running on vsock' in l for l in lines),
            'tcpForwardFailures': sum('failed to set up forwarding tcp' in l for l in lines),
            'tcpForwards': sum('forwarding tcp ' in l and 'not forwarding tcp ' not in l
                               and 'failed to set up forwarding tcp ' not in l for l in lines)}


def engine_peer_identity(path):
    uid, gid = ctypes.c_uint(), ctypes.c_uint()
    query = ctypes.CDLL(None, use_errno=True).getpeereid
    query.argtypes = [ctypes.c_int, ctypes.POINTER(ctypes.c_uint), ctypes.POINTER(ctypes.c_uint)]
    query.restype = ctypes.c_int
    with unix_sockets.socket(unix_sockets.AF_UNIX, unix_sockets.SOCK_STREAM) as stream:
        stream.settimeout(2)
        stream.connect(str(path))
        require(query(stream.fileno(), ctypes.byref(uid), ctypes.byref(gid)) == 0,
                'ENGINE_PEER_ID_READ_FAILED')
    return {'uid': uid.value, 'gid': gid.value}


def run(images, binary, root, evidence, command):
    require(os.environ.get('GITHUB_ACTIONS') == 'true' and platform.system() == 'Darwin'
            and platform.machine() == 'x86_64' and os.getuid() != 0,
            'STANDARD_INTEL_MACOS_CI_REQUIRED')
    require(command and not (Path.cwd() / '.env').exists(), 'CLEAN_CHECKOUT_AND_COMMAND_REQUIRED')
    require(root.is_absolute() and root.parent.resolve() == root.parent
            and not os.path.lexists(root) and len(os.fsencode(root)) <= 48
            and not os.path.lexists(evidence), 'FRESH_SHORT_OWNED_ROOTS_REQUIRED')
    require(shutil.disk_usage(root.parent).free >= 44 << 30, 'HOST_BUILD_AND_VM_RESERVE_REQUIRED')
    for tool in ('colima', 'docker', 'limactl'):
        require(shutil.which(tool) is not None, 'PINNED_CI_ENGINE_TOOLS_REQUIRED')
    root.mkdir(mode=0o700)
    evidence.mkdir(mode=0o700, parents=True)
    home = root / 'colima'
    # Colima ignores COLIMA_HOME unless that directory already exists.
    home.mkdir(mode=0o700)
    docker_config = root / 'docker-config'
    docker_config.mkdir(mode=0o700)
    environment = dict(os.environ, COLIMA_HOME=str(home), DOCKER_CONFIG=str(docker_config))
    environment.pop('LIMA_HOME', None)
    # A fresh COLIMA_HOME owns both this profile and its Lima data disk.
    colima = ['colima', '--profile', 'fixture']
    result = {'success': False, 'scope': 'Owned CI VM and canonical Broker command',
              'fullWorkspaceQualifiedByWrapper': False, 'mainExecuted': False,
              'virtualCpuCount': 2, 'virtualMemoryGiB': 4,
              'osDiskGiB': 8, 'containerDataDiskGiB': 16, 'xfsBackingGiB': 12}
    started = False

    def call(name, args, **kwargs):
        with (evidence / (name + '.log')).open('xb') as log:
            done = subprocess.run(args, env=environment, stdout=log,
                                  stderr=subprocess.STDOUT, **kwargs)
        result[name + 'ExitCode'] = done.returncode
        require(done.returncode == 0, name.upper() + '_FAILED')

    try:
        call('version', ['colima', 'version'])
        require(re.search(r'colima version v?0\.10\.3\b', (evidence / 'version.log').read_text()),
                'PINNED_COLIMA_VERSION_REQUIRED')
        started = True
        call('start', [*colima, 'start', '--template=false', '--vm-type', 'vz',
                      '--runtime', 'docker', '--cpus', '2', '--memory', '4',
                      '--root-disk', '8', '--disk', '16', '--mount', 'none',
                      '--ssh-agent=false', '--ssh-config=false', '--activate=false',
                      '--port-forwarder', 'ssh'], timeout=600)
        call('xfs', [*colima, 'ssh', '--', 'sh', '-s'], input=GUEST_SETUP.encode(), timeout=300)
        socket = home / 'fixture/docker.sock'
        require(socket.is_socket(), 'COLIMA_ENGINE_SOCKET_REQUIRED')
        try:
            result['engineSocketPeer'] = engine_peer_identity(socket)
        except Exception as error:
            result['enginePeerDiagnosticFailureClass'] = type(error).__name__
        with (evidence / 'pipeline.log').open('xb') as log:
            child = subprocess.Popen([sys.executable, '-I', '-B', str(HERE / 'with-ci-broker.py'),
                '--images', str(images), '--engine-socket', str(socket.resolve()),
                '--binary', str(binary), '--root', str(root / 'fixture'), '--command', *command],
                env=environment, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            try:
                result['pipelineExitCode'] = child.wait()
            finally:
                if child.poll() is None:
                    os.killpg(child.pid, signal.SIGINT)
                    child.wait(timeout=180)
        value = json.loads((root / 'fixture/result.json').read_bytes())
        result.update(mainExecuted=value['mainExecuted'], mainExitCode=value.get('mainExitCode'),
                      pipelineSuccess=value['success'])
        require(result['pipelineExitCode'] == 0 and value['success'], 'BROKER_OR_COMMAND_FAILED')
        result['success'] = True
    except Exception as error:
        result['failureClass'] = type(error).__name__
        code = str(error) if isinstance(error, ValueError) else ''
        result['failureCode'] = code if re.fullmatch('[A-Z_]+', code) else None
    finally:
        try:
            result['agentForwarding'] = agent_forwarding(home)
        except Exception as error:
            result['agentDiagnosticFailureClass'] = type(error).__name__
        # Never address the default profile, global Docker context, or another VM.
        if started:
            try:
                call('delete', [*colima, 'delete', '--force', '--data'], timeout=180)
                require(not os.path.lexists(home / 'fixture/docker.sock'), 'OWNED_ENGINE_SOCKET_REMAINS')
                result['ownedVmDeleted'] = True
            except Exception as error:
                result['success'] = False
                result['cleanupFailureClass'] = type(error).__name__
        for source, destination in ((root / 'fixture/result.json', 'pipeline-result.json'),
                (root / 'fixture/engine-events.jsonl', 'engine-events.jsonl'),
                (root / 'fixture/health-observations.jsonl', 'health-observations.jsonl'),
                (root / 'fixture/broker/evidence/result.json', 'broker-result.json'),
                (root / 'fixture/broker/evidence/command.log', 'wrapped-command.log'),
                (root / 'fixture/broker/evidence/broker.stdout', 'broker.stdout'),
                (root / 'fixture/broker/evidence/broker.stderr', 'broker.stderr'),
                (root / 'fixture/helper-result.json', 'helper-result.json')):
            if source.is_file():
                shutil.copyfile(source, evidence / destination)
        (evidence / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    return result


def main():
    def interrupted(signum, frame):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, interrupted)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--images', type=Path, required=True)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--evidence', type=Path, required=True)
    parser.add_argument('--command', nargs=argparse.REMAINDER, required=True)
    args = parser.parse_args()
    result = run(args.images, args.binary, args.root, args.evidence, args.command)
    print(json.dumps(result))
    return 0 if result['success'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
