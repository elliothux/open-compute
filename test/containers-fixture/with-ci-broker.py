#!/usr/bin/env python3
"""Load verified fixture images into an empty dedicated Engine and own one Broker run."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import resource
import shutil
import subprocess
import sys
import uuid

HERE = Path(__file__).resolve().parent


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


consumer = load('consume_images', 'consume-images.py')
helper = load('materialize_helper', 'materialize-helper.py')
broker = load('with_broker', 'with-broker.py')
require = consumer.oci.require


def event_projection(config):
    return ('{"timeNano":{{.TimeNano}},"action":{{json (index (split (printf "%%s" .Action) ":") 0)}},'
            '"role":{{if eq (index .Actor.Attributes "image") %s}}"qualification"'
            '{{else if eq (index .Actor.Attributes "image") %s}}"sidecar"{{else}}"other"{{end}},'
            '"exitCode":{{json (index .Actor.Attributes "exitCode")}}}'
            % (json.dumps(config['qualification_image']), json.dumps(config['sidecar_image'])))


def event_file_limit():
    resource.setrlimit(resource.RLIMIT_FSIZE, (1 << 20, 1 << 20))


def configuration(root, engine_socket, report, volume, policy):
    require(isinstance(policy, dict) and set(policy) == {
        'max_instances', 'max_snapshots', 'max_snapshot_bytes', 'resources'}, 'INPUT_POLICY_FIELDS')
    require(broker.prepare.integer(policy['max_instances'], 2, 1024)
            and broker.prepare.integer(policy['max_snapshots'], 1, 100000)
            and broker.prepare.integer(policy['max_snapshot_bytes'], 1, (1 << 63) - 1),
            'INPUT_LIMITS')
    resources = policy['resources']
    require(isinstance(resources, dict) and set(resources) == broker.prepare.RESOURCE_KEYS
            and all(broker.prepare.integer(resources[k], 1, (1 << 63) - 1)
                    for k in ('cpuNanos', 'memoryBytes', 'diskBytes'))
            and all(broker.prepare.integer(resources[k], 1, (1 << 32) - 1)
                    for k in ('pids', 'openFiles')), 'INPUT_RESOURCE_LIMITS')
    value = {'socket': str(root / 'broker.sock'), 'engine_socket': str(engine_socket),
             'peer_uid': os.getuid(),
             'sidecar_image': 'sha256:' + report['images']['sidecar']['configSha256'],
             'exec_helper_volume': volume, 'exec_helper_sha256': report['binaryInputs']['sidecarSha256'],
             'qualification_image': 'sha256:' + report['images']['qualification']['configSha256'],
             'max_leases': 4096, 'max_instances': policy['max_instances'], 'max_requests': 128,
             'relay_host': '127.0.0.1', 'state_dir': str(root / 'state'),
             'spool_dir': str(root / 'spool'), 'max_image_bytes': 2 << 30,
             'max_stream_bytes': 64 << 20, 'resources': resources}
    require(set(value) == broker.CONFIG_FIELDS and len(os.fsencode(value['socket'])) <= 103,
            'BROKER_CONFIG_OR_SOCKET_PATH')
    return value


def engine_profile(engine, architecture):
    status, value = engine.request('GET', '/info')
    require(status == 200 and value['OSType'] == 'linux'
            and value['Architecture'] in {'arm64': ('arm64', 'aarch64'),
                                          'amd64': ('amd64', 'x86_64')}[architecture]
            and value['Driver'] == 'overlay2' and value['CgroupDriver'] == 'cgroupfs'
            and value['CgroupVersion'] == '2'
            and ['Backing Filesystem', 'xfs'] in value['DriverStatus']
            and any('name=seccomp' in row for row in value['SecurityOptions']),
            'QUALIFIABLE_ENGINE_PROFILE_REQUIRED')
    return {k: value[k] for k in ('ID', 'OSType', 'Architecture', 'Driver',
                                 'DriverStatus', 'CgroupDriver', 'CgroupVersion', 'ServerVersion')}


def run(directory, engine_socket, binary, root, command):
    require(os.getuid() != 0 and command, 'ORDINARY_OWNER_AND_EXPLICIT_COMMAND_REQUIRED')
    require(root.is_absolute() and root.parent.resolve() == root.parent
            and not os.path.lexists(root), 'FRESH_PRIVATE_ROOT_REQUIRED')
    architecture = {'arm64': 'arm64', 'aarch64': 'arm64', 'x86_64': 'amd64'}.get(platform.machine())
    require(architecture is not None, 'NATIVE_ARCHITECTURE_REQUIRED')
    _, _, raw = consumer.oci.regular(directory / 'images.json', 65536, True)
    consumer.consume(directory, architecture)
    require(consumer.oci.regular(directory / 'images.json', 65536, True)[2] == raw,
            'ARTIFACT_REPORT_CHANGED')
    report = json.loads(raw, object_pairs_hook=consumer.oci.pairs)
    _, _, raw = consumer.oci.regular(HERE / 'policy.json', 65536, True)
    policy = json.loads(raw, object_pairs_hook=consumer.oci.pairs)
    volume = 'oc-ci-helper-' + uuid.uuid4().hex
    config = configuration(root / 'broker', engine_socket, report, volume, policy)
    require(binary.is_absolute() and binary.resolve() == binary
            and binary.stat().st_mode & 0o111, 'CANONICAL_EXECUTABLE_REQUIRED')
    binary_sha, _, _ = consumer.oci.regular(binary, 1 << 30)
    docker = shutil.which('docker')
    require(docker is not None, 'DOCKER_CLI_REQUIRED')
    engine = helper.Engine(engine_socket)
    anchor = engine_profile(engine, architecture)
    before = engine.inventory()
    require(all(not before[k] for k in ('containers', 'images', 'volumes')),
            'EMPTY_DEDICATED_ENGINE_REQUIRED')
    root.mkdir(mode=0o700)
    (root / 'docker-config').mkdir(mode=0o700)
    env = {'PATH': os.environ.get('PATH', os.defpath), 'HOME': str(root),
           'DOCKER_CONFIG': str(root / 'docker-config')}
    result = {'success': False, 'scope': 'Fixture image/helper/Broker lifecycle and explicit command',
              'fullWorkspaceQualifiedByWrapper': False, 'mainExecuted': False,
              'engineProfile': {k: anchor[k] for k in ('OSType', 'Architecture', 'Driver',
                  'DriverStatus', 'CgroupDriver', 'CgroupVersion', 'ServerVersion')}}
    materialized = False
    observer = None
    event_output = None
    health_observer = None
    health_output = None
    try:
        try:
            status, version = engine.request('GET', '/version')
            components = version.get('Components') if status == 200 and isinstance(version, dict) else None
            result['engineRuntimeVersions'] = {
                row['Name']: row['Version'] for row in components[:32]
                if isinstance(row, dict) and row.get('Name') in ('Engine', 'containerd', 'runc')
                and isinstance(row.get('Version'), str)
                and re.fullmatch(r'v?\d+\.\d+\.\d+(?:[-+][A-Za-z0-9_.-]{1,64})?', row['Version'])
            } if isinstance(components, list) else {}
        except Exception as error:
            result['engineRuntimeVersionsFailureClass'] = type(error).__name__
        for name in ('sidecar', 'qualification'):
            archive = directory / (name + '.docker.tar')
            fd = os.open(archive, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
            with os.fdopen(fd, 'rb') as stream, (root / (name + '-load.log')).open('xb') as log:
                before_file = consumer.oci.identity(os.fstat(stream.fileno()))
                expected = report['dockerArchives'][name]['sha256']
                require(hashlib.file_digest(stream, 'sha256').hexdigest() == expected
                        and before_file == consumer.oci.identity(archive.lstat()),
                        'BOOTSTRAP_ARCHIVE_CHANGED')
                stream.seek(0)
                subprocess.run([docker, '--host', 'unix://' + str(engine_socket), 'image', 'load'],
                               stdin=stream, env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
                stream.seek(0)
                require(hashlib.file_digest(stream, 'sha256').hexdigest() == expected
                        and before_file == consumer.oci.identity(os.fstat(stream.fileno()))
                        == consumer.oci.identity(archive.lstat()), 'BOOTSTRAP_ARCHIVE_CHANGED')
        wanted = sorted({'sha256:' + report['images'][name]['configSha256']
                         for name in ('sidecar', 'qualification')})
        loaded = engine.inventory()
        require(loaded == dict(before, images=wanted), 'BOOTSTRAP_IMAGE_INVENTORY_MISMATCH')
        proof = helper.materialize(engine, config['sidecar_image'], config['exec_helper_sha256'],
                                   volume, architecture)
        (root / 'helper-result.json').write_text(json.dumps(proof, indent=2) + '\n')
        require(proof['success'] and proof['allOwnedContainersAbsent'] and proof['volumeRetained'],
                'HELPER_MATERIALIZATION_FAILED')
        materialized = True
        profile = root / 'broker'
        profile.mkdir(mode=0o700)
        text = '\n'.join(k + ' = ' + json.dumps(v) for k, v in config.items() if k != 'resources')
        text += '\n[resources]\n' + '\n'.join(k + ' = ' + str(v) for k, v in config['resources'].items())
        path = profile / 'broker.toml'
        with path.open('x') as stream:
            os.fchmod(stream.fileno(), 0o600)
            stream.write(text + '\n')
        event_output = (root / 'engine-events.jsonl').open('xb')
        # Select fields before writing; exec commands, IDs, names and labels never enter the log.
        observer = subprocess.Popen([docker, '--host', 'unix://' + str(engine_socket),
            'events', '--filter', 'type=container', '--format', event_projection(config)],
            env=env, stdout=event_output, stderr=subprocess.DEVNULL, preexec_fn=event_file_limit)
        health_output = (root / 'health-observations.jsonl').open('xb')
        health_observer = subprocess.Popen([sys.executable, '-I', '-B', str(HERE / 'observe-health.py'),
            '--engine-socket', str(engine_socket), '--image', config['qualification_image']],
            env=env, stdout=health_output, stderr=subprocess.DEVNULL, preexec_fn=event_file_limit)
        with (root / 'broker-command.log').open('xb') as log:
            done = subprocess.run([sys.executable, '-I', '-B', str(HERE / 'with-broker.py'),
                '--binary', str(binary), '--binary-sha256', binary_sha, '--config', str(path),
                '--policy', str(HERE / 'policy.json'), '--images', str(directory / 'one'),
                str(directory / 'two'), '--command', *command],
                stdout=log, stderr=subprocess.STDOUT, env=os.environ)
        value = json.loads((profile / 'evidence/result.json').read_bytes())
        result.update(brokerWrapperExitCode=done.returncode, mainExecuted=value['mainExecuted'],
                      mainExitCode=value.get('mainExitCode'))
        require(done.returncode == 0 and value['success'], 'BROKER_OR_COMMAND_FAILED')
        result['success'] = True
    except Exception as error:
        result['failureClass'] = type(error).__name__
        code = str(error) if isinstance(error, ValueError) else ''
        result['failureCode'] = code if re.fullmatch('[A-Z_]+', code) else None
    finally:
        if health_observer is not None:
            try:
                result['healthObserverCleanup'] = broker.stop_child(health_observer)
            except Exception as error:
                result['success'] = False
                result['healthObserverCleanupFailureClass'] = type(error).__name__
        if health_output is not None:
            health_output.close()
            result['healthObservationLogBytes'] = (root / 'health-observations.jsonl').stat().st_size
        if observer is not None:
            try:
                result['eventObserverCleanup'] = broker.stop_child(observer)
            except Exception as error:
                result['success'] = False
                result['eventObserverCleanupFailureClass'] = type(error).__name__
        if event_output is not None:
            event_output.close()
            result['engineEventLogBytes'] = (root / 'engine-events.jsonl').stat().st_size
        try:
            after = engine.inventory()
            require(not after['containers'] and after['networks'] == before['networks'],
                    'COMMAND_ENGINE_RESOURCE_LEAK')
            if materialized:
                status, value = engine.request('GET', '/volumes/' + volume)
                require(status == 200 and value['Name'] == volume and value['Driver'] == 'local'
                        and not value.get('Options') and value['Labels'] == {
                            'dev.open-compute.exec-helper.image': config['sidecar_image'],
                            'dev.open-compute.exec-helper.sha256': config['exec_helper_sha256']},
                        'OWNED_HELPER_VOLUME_CHANGED')
                require(engine.request('DELETE', '/volumes/' + volume)[0] == 204,
                        'OWNED_HELPER_VOLUME_DELETE')
            after = engine.inventory()
            require(not after['containers'] and not after['volumes']
                    and after['networks'] == before['networks']
                    and engine_profile(engine, architecture) == anchor, 'ENGINE_POSTFLIGHT')
            result.update(ownedHelperVolumeAbsent=True, engineAnchorUnchanged=True,
                          retainedImageCount=len(after['images']))
        except Exception as error:
            result['success'] = False
            result['cleanupFailureClass'] = type(error).__name__
            code = str(error) if isinstance(error, ValueError) else ''
            result['cleanupFailureCode'] = code if re.fullmatch('[A-Z_]+', code) else None
        (root / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--images', required=True, type=Path)
    parser.add_argument('--engine-socket', required=True, type=Path)
    parser.add_argument('--binary', required=True, type=Path)
    parser.add_argument('--root', required=True, type=Path)
    parser.add_argument('--command', nargs=argparse.REMAINDER, required=True)
    args = parser.parse_args()
    result = run(args.images, args.engine_socket, args.binary, args.root, args.command)
    print(json.dumps(result))
    return 0 if result['success'] else 1


if __name__ == '__main__':
    try:
        raise SystemExit(main())
    except Exception as error:
        code = str(error) if isinstance(error, ValueError) else ''
        print(json.dumps({'success': False, 'failureClass': type(error).__name__,
                          'failureCode': code if re.fullmatch('[A-Z_]+', code) else None}))
        raise SystemExit(1)
