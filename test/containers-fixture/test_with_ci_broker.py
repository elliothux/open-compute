"""Synthetic orchestration and failure checks; no Engine or Broker is started."""
import copy
import importlib.util
import json
from pathlib import Path
import resource
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import test_consume_images as consume_tests

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('with_ci_broker', HERE / 'with-ci-broker.py')
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class Events(unittest.TestCase):
    def test_output_limit_bounds_the_child_without_changing_the_parent(self):
        parent = HERE.parent.parent / '.temp/engine-event-tests'
        parent.mkdir(parents=True, exist_ok=True)
        limits = resource.getrlimit(resource.RLIMIT_FSIZE)
        with tempfile.TemporaryDirectory(dir=parent) as directory:
            output = Path(directory) / 'bounded.log'
            with output.open('xb') as log:
                done = subprocess.run([sys.executable, '-I', '-B', '-c',
                    'import sys; sys.stdout.buffer.write(b"x" * (2 << 20)); sys.stdout.flush()'],
                    stdout=log, stderr=subprocess.DEVNULL, preexec_fn=fixture.event_file_limit)
            self.assertNotEqual(done.returncode, 0)
            self.assertEqual(output.stat().st_size, 1 << 20)
        self.assertEqual(resource.getrlimit(resource.RLIMIT_FSIZE), limits)


class Pipeline(unittest.TestCase):
    def setUp(self):
        self.data = consume_tests.Consume('test_complete_artifact_emits_only_two_images_without_runtime_authority')
        self.data.setUp()
        self.root = self.data.root
        self.output = self.root / 'run'
        self.binary = self.root / 'ocd'
        self.binary.write_bytes(b'controlled executable; never executed')
        self.binary.chmod(0o755)
        self.report = self.data.report
        self.config = None
        self.calls = []
        self.materializations = 0
        self.main_exit = 0
        self.tamper_archive = False
        self.leak = False
        self.version_failure = False
        self.info = {'ID': 'controlled-engine', 'OSType': 'linux', 'Architecture': 'aarch64',
                     'Driver': 'overlay2', 'CgroupDriver': 'cgroupfs', 'CgroupVersion': '2',
                     'DriverStatus': [['Backing Filesystem', 'xfs']],
                     'SecurityOptions': ['name=seccomp,profile=builtin'], 'ServerVersion': '29.4.0'}
        self.inventory = {'containers': [], 'images': [], 'networks': ['controlled-default'], 'volumes': []}
        owner = self

        class Engine:
            def __init__(self, path):
                self.path = path

            def inventory(self):
                return copy.deepcopy(owner.inventory)

            def request(self, method, path):
                owner.calls.append((method, path))
                if (method, path) == ('GET', '/info'):
                    return 200, owner.info
                if (method, path) == ('GET', '/version'):
                    if owner.version_failure:
                        raise TimeoutError('private transport details')
                    return 200, {'Components': [
                        {'Name': 'Engine', 'Version': '29.5.2'},
                        {'Name': 'containerd', 'Version': '2.2.1'},
                        {'Name': 'runc', 'Version': '1.3.5'},
                        {'Name': 'private component', 'Version': '1.2.3'},
                        {'Name': 'runc', 'Version': 'private-version-and-token'},
                        {'Name': 'Engine', 'Version': True}], 'Env': ['private-token']}
                if method == 'GET' and path.startswith('/volumes/'):
                    return 200, {'Name': path.removeprefix('/volumes/'), 'Driver': 'local',
                                 'Options': None, 'Labels': {
                                     'dev.open-compute.exec-helper.image': owner.config['sidecar_image'],
                                     'dev.open-compute.exec-helper.sha256': owner.config['exec_helper_sha256']}}
                if method == 'DELETE' and path.startswith('/volumes/'):
                    owner.inventory['volumes'].remove(path.removeprefix('/volumes/'))
                    return 204, None
                raise AssertionError('Unexpected controlled Engine call')

        def materialize(engine, image, helper_sha, volume, architecture):
            self.materializations += 1
            self.inventory['volumes'].append(volume)
            self.config = {'sidecar_image': image, 'exec_helper_sha256': helper_sha}
            self.assertEqual(architecture, 'arm64')
            return {'success': True, 'allOwnedContainersAbsent': True, 'volumeRetained': True}

        def run(args, **kwargs):
            self.calls.append(tuple(args))
            if args[0] == '/controlled/docker':
                loaded = len(self.inventory['images'])
                name = ('sidecar', 'qualification')[loaded]
                self.assertEqual(args[-2:], ['image', 'load'])
                data = kwargs['stdin'].read()
                self.assertEqual(data, (self.root / (name + '.docker.tar')).read_bytes())
                self.inventory['images'].append('sha256:' + self.report['images'][name]['configSha256'])
                self.inventory['images'].sort()
                if self.tamper_archive:
                    with (self.root / (name + '.docker.tar')).open('ab') as stream:
                        stream.write(b'controlled concurrent change')
                return subprocess.CompletedProcess(args, 0)
            self.assertEqual(args[3], str(HERE / 'with-broker.py'))
            self.assertEqual(args[args.index('--command') + 1:], ['controlled-gate', '--unchanged-scope'])
            evidence = Path(args[args.index('--config') + 1]).parent / 'evidence'
            evidence.mkdir()
            value = {'success': self.main_exit == 0, 'mainExecuted': True, 'mainExitCode': self.main_exit}
            (evidence / 'result.json').write_text(json.dumps(value))
            if self.leak:
                self.inventory['containers'].append('controlled-remains')
            return subprocess.CompletedProcess(args, self.main_exit)

        class Observer:
            pid = 12345

            def __init__(self, args, **kwargs):
                owner.calls.append(tuple(args))
                if args[0] == sys.executable:
                    owner.assertEqual(args[1:4], ['-I', '-B', str(HERE / 'observe-health.py')])
                    owner.assertEqual(args[-2:], ['--image', 'sha256:' + owner.report['images']['qualification']['configSha256']])
                else:
                    owner.assertEqual(args[-4:-2], ['--filter', 'type=container'])
                owner.assertIs(kwargs['preexec_fn'], fixture.event_file_limit)
                owner.assertEqual(kwargs['stderr'], subprocess.DEVNULL)
                self.returncode = None

            def poll(self):
                return self.returncode

            def terminate(self):
                self.returncode = -15

            def wait(self, **kwargs):
                return self.returncode

        self.patches = [patch.object(fixture.platform, 'machine', return_value='arm64'),
                        patch.object(fixture.shutil, 'which', return_value='/controlled/docker'),
                        patch.object(fixture.helper, 'Engine', Engine),
                        patch.object(fixture.helper, 'materialize', materialize),
                        patch.object(fixture.subprocess, 'Popen', Observer),
                        patch.object(fixture.subprocess, 'run', run)]
        for value in self.patches:
            value.start()

    def tearDown(self):
        for value in reversed(self.patches):
            value.stop()
        self.data.tearDown()

    def run_pipeline(self):
        return fixture.run(self.root, Path('/controlled/engine.sock'), self.binary,
                           self.output, ['controlled-gate', '--unchanged-scope'])

    def test_load_materialize_canonical_wrapper_and_exact_volume_cleanup(self):
        result = self.run_pipeline()
        self.assertTrue(result['success'])
        self.assertTrue(result['mainExecuted'])
        self.assertEqual(result['mainExitCode'], 0)
        self.assertEqual(self.materializations, 1)
        self.assertEqual(self.inventory['volumes'], [])

        self.assertEqual(len(self.inventory['images']), 2)
        self.assertFalse(result['fullWorkspaceQualifiedByWrapper'])
        self.assertTrue(result['eventObserverCleanup']['reaped'])
        self.assertTrue(result['healthObserverCleanup']['reaped'])
        self.assertEqual(result['engineProfile']['ServerVersion'], '29.4.0')
        self.assertEqual(result['engineRuntimeVersions'],
                         {'Engine': '29.5.2', 'containerd': '2.2.1', 'runc': '1.3.5'})
        self.assertNotIn('private', json.dumps(result))
        config = self.output / 'broker/broker.toml'
        self.assertEqual(config.stat().st_mode & 0o777, 0o600)
        self.assertEqual(config.parent.stat().st_mode & 0o777, 0o700)

    def test_progress_capture_failure_reaps_resources_without_running_the_command(self):
        with patch.object(fixture.broker, 'relay_gate_progress',
                          side_effect=ValueError('GATE_PROGRESS_LOG_REQUIRED')):
            result = self.run_pipeline()
        self.assertFalse(result['success'])
        self.assertFalse(result['mainExecuted'])
        self.assertEqual(result['failureCode'], 'GATE_PROGRESS_LOG_REQUIRED')
        self.assertTrue(result['ownedHelperVolumeAbsent'])
        self.assertFalse(self.inventory['volumes'])
        self.assertTrue(result['eventObserverCleanup']['reaped'])
        self.assertTrue(result['healthObserverCleanup']['reaped'])
        self.assertFalse(any(isinstance(row, tuple) and len(row) > 3
                             and row[3] == str(HERE / 'with-broker.py') for row in self.calls))

    def test_version_observation_failure_preserves_command_and_cleanup(self):
        self.version_failure = True
        result = self.run_pipeline()
        self.assertTrue(result['success'])
        self.assertTrue(result['mainExecuted'])
        self.assertEqual(result['mainExitCode'], 0)
        self.assertEqual(result['engineRuntimeVersionsFailureClass'], 'TimeoutError')
        self.assertNotIn('private transport details', json.dumps(result))
        self.assertTrue(result['healthObserverCleanup']['reaped'])
        self.assertEqual(self.inventory['volumes'], [])

    def test_operator_engine_is_rejected_before_any_mutation(self):
        self.inventory['images'].append('controlled-existing-image')
        with self.assertRaisesRegex(ValueError, '^EMPTY_DEDICATED_ENGINE_REQUIRED$'):
            self.run_pipeline()
        self.assertFalse(self.output.exists())
        self.assertEqual(self.materializations, 0)
        self.assertFalse(any(call[0] == '/controlled/docker' for call in self.calls))

    def test_wrong_cgroup_driver_is_not_accepted_as_qualified(self):
        self.info['CgroupDriver'] = 'systemd'
        with self.assertRaisesRegex(ValueError, '^QUALIFIABLE_ENGINE_PROFILE_REQUIRED$'):
            self.run_pipeline()
        self.assertFalse(self.output.exists())

    def test_writable_disk_profile_requires_xfs_before_mutation(self):
        self.info['DriverStatus'] = [['Backing Filesystem', 'extfs']]
        with self.assertRaisesRegex(ValueError, '^QUALIFIABLE_ENGINE_PROFILE_REQUIRED$'):
            self.run_pipeline()
        self.assertFalse(self.output.exists())

    def test_archive_tamper_is_rejected_before_engine_contact(self):
        with (self.root / 'sidecar.docker.tar').open('ab') as stream:
            stream.write(b'controlled change')
        with self.assertRaisesRegex(ValueError, '^ARTIFACT_ARCHIVE_MISMATCH$'):
            self.run_pipeline()
        self.assertEqual(self.calls, [])

    def test_held_archive_change_stops_before_materialization(self):
        self.tamper_archive = True
        result = self.run_pipeline()
        self.assertFalse(result['success'])
        self.assertEqual(result['failureCode'], 'BOOTSTRAP_ARCHIVE_CHANGED')
        self.assertEqual(self.materializations, 0)
        self.assertFalse(result['mainExecuted'])

    def test_failed_gate_is_not_hidden_and_owned_volume_is_retired(self):
        self.main_exit = 1
        result = self.run_pipeline()
        self.assertFalse(result['success'])
        self.assertEqual(result['mainExitCode'], 1)
        self.assertEqual(result['failureCode'], 'BROKER_OR_COMMAND_FAILED')
        self.assertTrue(result['ownedHelperVolumeAbsent'])
        self.assertEqual(self.inventory['volumes'], [])
        self.assertTrue(result['eventObserverCleanup']['reaped'])
        self.assertTrue(result['healthObserverCleanup']['reaped'])

    def test_container_leak_retains_volume_and_reports_cleanup_failure(self):
        self.main_exit = 1
        self.leak = True
        result = self.run_pipeline()
        self.assertFalse(result['success'])
        self.assertEqual(result['cleanupFailureCode'], 'COMMAND_ENGINE_RESOURCE_LEAK')
        self.assertEqual(len(self.inventory['volumes']), 1)
        self.assertFalse(any(call[0] == 'DELETE' for call in self.calls))


if __name__ == '__main__':
    unittest.main()
