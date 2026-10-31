"""Exercise the CI Engine lifecycle with command doubles and no privileged changes."""
import ast
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]

DOUBLE = r'''#!PYTHON
import json, os
from pathlib import Path
import sys
name = Path(sys.argv[0]).name
args = sys.argv[1:]
state = Path(os.environ['DOUBLE_STATE'])
with (state / 'calls.jsonl').open('a') as out:
    out.write(json.dumps([name, *args]) + '\n')
fail = os.environ.get('FAIL_STAGE', '')
if name == 'uname': print('Linux')
elif name == 'id': print('1001')
elif name == 'sudo':
    if args[1:3] == ['python3', '-I'] and os.environ.get('IDLE_SDK') == '1':
        if os.environ.get('SDK_BUSY') == '1': sys.exit(1)
        (state / 'sdk-reclaimed').touch()
    if args[1:3] == ['systemctl', 'stop'] and args[-1].startswith('oc-ci-engine-'):
        evidence = Path(os.environ['DOUBLE_EVIDENCE'])
        (state / 'pre-cleanup-evidence.json').write_text(json.dumps({
            'resultCopied': (evidence / 'result.json').is_file(),
            'healthCopied': (evidence / 'health-observations.jsonl').is_file(),
            'finalResultExists': (evidence / 'engine-result.json').exists(),
        }))
        if fail == 'stop': sys.exit(1)
    if args[1:3] == ['systemctl', 'stop'] and args[-1].endswith('.slice'):
        if fail == 'limits-stop': sys.exit(1)
    if args[1:3] == ['systemctl', 'is-active']: sys.exit(0 if fail == 'stop' else 3)
    if args[1] == 'tee' and fail == 'cgroup-kill': sys.exit(1)
    if args[1] == 'mount':
        if fail == 'mount': sys.exit(1)
        (state / 'mounted').touch()
    if args[1] == 'umount': (state / 'mounted').unlink()
    if args[1] == 'systemd-run': (state / 'engine-started').touch()
    if args[1:3] == ['systemctl', 'set-property']:
        (state / 'runtime-overrides').touch()
        if fail in ('limits', 'limits-stop'): sys.exit(1)
    if args[1:3] == ['systemctl', 'revert']: (state / 'runtime-overrides').unlink(missing_ok=True)
elif name == 'cp':
    if fail == 'copy-evidence': sys.exit(1)
    import shutil
    shutil.copyfile(*args)
elif name == 'timeout':
    import subprocess
    sys.exit(subprocess.run(args[1:]).returncode)
elif name == 'findmnt':
    if not (state / 'mounted').exists(): sys.exit(1)
    print('xfs rw,prjquota')
elif name == 'docker':
    if 'network' in args:
        rows = [dict(Id='a' * 64, Name='bridge', Driver='bridge', Scope='local',
                     Created='before', Options={'com.docker.network.bridge.default_bridge': 'true'},
                     IPAM={'Config': [{'Subnet': '172.17.0.0/16'}]}),
                dict(Id='b' * 64, Name='owned-network', Driver='bridge', Scope='local',
                     Created='stable', Options={}, IPAM={'Config': []})]
        if (state / 'engine-started').exists():
            if fail.startswith('bridge-'):
                rows[0].update(Id='c' * 64, Created='after')
            if fail == 'bridge-config': rows[0]['IPAM']['Config'][0]['Subnet'] = '172.18.0.0/16'
            if fail == 'custom-network-id': rows[1]['Id'] = 'd' * 64
            if fail == 'network-added': rows.append(dict(rows[1], Id='e' * 64, Name='unexpected'))
        if 'inspect' in args:
            if fail == 'network-query': sys.exit(1)
            print(json.dumps(rows))
        else: print('\n'.join(row['Id'] for row in rows))
    elif 'container' in args:
        if fail == 'inventory-query': sys.exit(1)
        if os.environ.get('EXISTING_CONTAINER') == '1': print('preserved-container')
    elif '--format' in args: print('original-engine')
    elif 'ls' in args:
        if fail == 'inventory' and (state / 'engine-started').exists(): print('changed')
    elif not (state / 'engine-started').exists(): sys.exit(1)
elif name == 'python3':
    if args[:2] == ['-I', '-B']:
        root = Path(args[args.index('--root') + 1]); root.mkdir()
        code = 1 if fail == 'pipeline' else 0
        (root / 'result.json').write_text(json.dumps({'success': code == 0}))
        (root / 'health-observations.jsonl').write_text('{"status":"starting"}\n')
        sys.exit(code)
    source = sys.stdin.read()
    if fail == 'result-write' and 'engine-result.json' in source: sys.exit(1)
    sys.argv = ['-', *args[1:]]
    from unittest.mock import patch
    from types import SimpleNamespace
    from contextlib import ExitStack
    def disk_usage(_):
        free = 1 << 30 if os.environ.get('LOW_DISK') == '1' else 50 << 30
        if (state / 'sdk-reclaimed').exists(): free = 50 << 30
        return SimpleNamespace(total=100 << 30, free=free)
    with ExitStack() as stack:
        if os.environ.get('IDLE_SDK') == '1':
            sdk = Path('/usr/local/lib/android')
            sdk_paths = {sdk, Path('/usr/share/dotnet'), Path('/usr/local/.ghcup')}
            exists, lstat, resolve = Path.exists, Path.lstat, Path.resolve
            stack.enter_context(patch.object(Path, 'exists', autospec=True,
                side_effect=lambda path: path == sdk if path in sdk_paths else exists(path)))
            stack.enter_context(patch.object(Path, 'lstat', autospec=True,
                side_effect=lambda path: SimpleNamespace(st_mode=0o40755, st_uid=0,
                    st_dev=1, st_ino=2) if path == sdk else lstat(path)))
            stack.enter_context(patch.object(Path, 'resolve', autospec=True,
                side_effect=lambda path: path if path == sdk else resolve(path)))
        stack.enter_context(patch('shutil.disk_usage', side_effect=disk_usage))
        exec(compile(source, '<controlled-wrapper>', 'exec'))
'''


class LinuxEngine(unittest.TestCase):
    def setUp(self):
        parent = ROOT / '.temp/linux-engine-tests'
        parent.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=parent)
        self.state = Path(self.temp.name).resolve()
        self.tools = self.state / 'bin'
        self.tools.mkdir()
        executable = self.tools / 'double'
        executable.write_text(DOUBLE.replace('PYTHON', sys.executable, 1))
        executable.chmod(0o755)
        for name in ('uname', 'id', 'sudo', 'systemd-run', 'systemctl', 'timeout',
                     'mount', 'umount', 'findmnt', 'docker', 'dockerd', 'mkfs.xfs',
                     'truncate', 'python3', 'cp'):
            (self.tools / name).symlink_to(executable)
        self.root = Path('/tmp') / ('oc-ci-unit-' + self.state.name[-8:])
        self.evidence = self.state / 'evidence'
        self.env = dict(os.environ, PATH=str(self.tools) + os.pathsep + os.environ['PATH'],
                        DOUBLE_STATE=str(self.state), DOUBLE_EVIDENCE=str(self.evidence),
                        GITHUB_ACTIONS='true')

    def tearDown(self):
        shutil.rmtree(self.root, ignore_errors=True)
        self.temp.cleanup()

    def execute(self, *, lifecycle=False):
        entry = str(HERE / 'with-ci-linux-engine.sh')
        invocation = (['bash', '-c', 'source "$1"; shift; run_linux_engine "$@"',
                       'native-lifecycle', entry] if lifecycle else ['bash', entry])
        return subprocess.run([*invocation,
            '--images', str(self.state / 'images'), '--binary', str(self.state / 'ocd'),
            '--root', str(self.root), '--evidence', str(self.evidence),
            '--command', 'controlled-gate', '--unchanged-scope'], cwd=ROOT,
            env=self.env, capture_output=True, text=True)

    def calls(self):
        return [json.loads(line) for line in (self.state / 'calls.jsonl').read_text().splitlines()]

    def test_low_disk_records_admission_before_refusing_without_engine_mutation(self):
        self.env.update(LOW_DISK='1', RUNNER_ENVIRONMENT='self-hosted')
        done = self.execute()
        self.assertNotEqual(done.returncode, 0)
        self.assertIn('CI_ENGINE_DISK_ADMISSION_FAILED', done.stderr)
        admission = json.loads((self.evidence / 'disk-admission.json').read_text())
        self.assertEqual(admission['initialFreeBytes'], 1 << 30)
        self.assertEqual(admission['finalFreeBytes'], 1 << 30)
        self.assertEqual(admission['requiredFreeBytes'], (44 << 30) + (100 << 30) // 20 + 1)
        self.assertEqual(admission['removedPreinstalledSdks'], [])
        self.assertFalse(any(call[0] == 'sudo' for call in self.calls()))
        self.assertFalse(self.root.exists())

    def test_hosted_runner_reclaims_only_until_the_original_budget_is_available(self):
        self.env.update(LOW_DISK='1', IDLE_SDK='1', RUNNER_ENVIRONMENT='github-hosted')
        done = self.execute()
        self.assertEqual(done.returncode, 0, done.stderr)
        admission = json.loads((self.evidence / 'disk-admission.json').read_text())
        self.assertEqual(admission['initialFreeBytes'], 1 << 30)
        self.assertEqual(admission['finalFreeBytes'], 50 << 30)
        self.assertEqual(admission['removedPreinstalledSdks'], [{
            'path': '/usr/local/lib/android', 'freeBytesBefore': 1 << 30,
            'freeBytesAfter': 50 << 30}])
        cleanup = [call for call in self.calls() if call[:4] == ['sudo', '-n', 'python3', '-I']]
        self.assertEqual(len(cleanup), 1)
        self.assertEqual(cleanup[0][-3:], ['/usr/local/lib/android', '1', '2'])

    def test_busy_sdk_is_preserved_and_insufficient_disk_still_refuses_the_engine(self):
        self.env.update(LOW_DISK='1', IDLE_SDK='1', SDK_BUSY='1', RUNNER_ENVIRONMENT='github-hosted')
        self.assertNotEqual(self.execute().returncode, 0)
        admission = json.loads((self.evidence / 'disk-admission.json').read_text())
        self.assertEqual(admission['removedPreinstalledSdks'], [])
        self.assertEqual(admission['rejectedPreinstalledSdks'], [{
            'path': '/usr/local/lib/android', 'reason': 'inspection-or-removal', 'exitCode': 1}])
        self.assertFalse(self.root.exists())

    def test_sdk_removal_rejects_live_references_mounts_and_path_replacement(self):
        source = (HERE / 'with-ci-linux-engine.sh').read_text().split("<<'PY'", 1)[1].split('\nPY', 1)[0]
        tree = ast.parse(source)
        cleanup = next(keyword.value.value for node in ast.walk(tree)
                       if isinstance(node, ast.Call) for keyword in node.keywords
                       if keyword.arg == 'input' and isinstance(keyword.value, ast.Constant))
        sdk = '/usr/local/lib/android'
        info = SimpleNamespace(st_dev=1, st_ino=2, st_mode=stat.S_IFDIR | 0o755, st_uid=0)
        for fault in ('none', 'fd', 'maps', 'mount', 'inode', 'symlink'):
            with self.subTest(fault=fault):
                def read_text(path):
                    if path.name == 'mountinfo':
                        return f'1 2 3 4 {sdk}/mounted rw' if fault == 'mount' else ''
                    return f'1 2 3 4 5 {sdk}/lib.so' if fault == 'maps' else ''

                with (patch.object(sys, 'argv', ['-', sdk, '1', '3' if fault == 'inode' else '2']),
                      patch.object(Path, 'lstat', return_value=info),
                      patch.object(Path, 'resolve', autospec=True,
                          side_effect=lambda path: Path('/changed') if fault == 'symlink' else path),
                      patch.object(Path, 'glob', return_value=[Path('/proc/123')]),
                      patch.object(Path, 'iterdir', return_value=iter([Path('/proc/123/fd/4')])),
                      patch.object(Path, 'read_text', autospec=True, side_effect=read_text),
                      patch('os.readlink', return_value=f'{sdk}/lib.so' if fault == 'fd' else '/usr/bin/python3'),
                      patch('shutil.rmtree') as remove):
                    if fault == 'none':
                        exec(compile(cleanup, '<sdk-cleanup>', 'exec'), {})
                        remove.assert_called_once_with(Path(sdk))
                    else:
                        with self.assertRaises(AssertionError):
                            exec(compile(cleanup, '<sdk-cleanup>', 'exec'), {})
                        remove.assert_not_called()

    def test_native_lifecycle_reuses_body_without_claiming_actions_identity(self):
        self.env['GITHUB_ACTIONS'] = 'false'
        done = self.execute(lifecycle=True)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertTrue(json.loads((self.evidence / 'engine-result.json').read_text())['success'])
        self.assertFalse((self.state / 'mounted').exists())
        self.assertFalse((self.state / 'runtime-overrides').exists())

    def test_default_bridge_recreation_preserves_configuration(self):
        self.env['FAIL_STAGE'] = 'bridge-id'
        done = self.execute()
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertTrue(json.loads((self.evidence / 'engine-result.json').read_text())['success'])

    def test_default_bridge_configuration_change_fails_restoration(self):
        self.env['FAIL_STAGE'] = 'bridge-config'
        self.assertNotEqual(self.execute().returncode, 0)
        self.assertEqual(json.loads((self.evidence / 'engine-result.json').read_text())[
            'cleanupExitCode'], 1)

    def test_custom_network_identity_change_fails_restoration(self):
        self.env['FAIL_STAGE'] = 'custom-network-id'
        self.assertNotEqual(self.execute().returncode, 0)
        self.assertEqual(json.loads((self.evidence / 'engine-result.json').read_text())[
            'cleanupExitCode'], 1)

    def test_new_network_fails_restoration(self):
        self.env['FAIL_STAGE'] = 'network-added'
        self.assertNotEqual(self.execute().returncode, 0)
        self.assertEqual(json.loads((self.evidence / 'engine-result.json').read_text())[
            'cleanupExitCode'], 1)

    def test_network_inspection_failure_exits_before_mutation(self):
        self.env['FAIL_STAGE'] = 'network-query'
        self.assertNotEqual(self.execute().returncode, 0)
        self.assertFalse(any(call[0] == 'sudo' for call in self.calls()))
        self.assertFalse(self.root.exists())

    def test_success_preserves_command_and_health_and_restores_default_engine(self):
        done = self.execute()
        self.assertEqual(done.returncode, 0, done.stderr)
        result = json.loads((self.evidence / 'engine-result.json').read_text())
        self.assertTrue(result['success'])
        self.assertEqual(json.loads((self.state / 'pre-cleanup-evidence.json').read_text()),
                         dict(resultCopied=True, healthCopied=True, finalResultExists=False))
        self.assertFalse(result['fullWorkspaceQualifiedByWrapper'])
        self.assertFalse((self.state / 'mounted').exists())
        self.assertFalse((self.state / 'runtime-overrides').exists())
        self.assertEqual((self.evidence / 'health-observations.jsonl').read_text(),
                         '{"status":"starting"}\n')
        calls = self.calls()
        pipeline = next(call for call in calls if 'with-ci-broker.py' in ' '.join(call))
        self.assertEqual(pipeline[pipeline.index('--command') + 1:],
                         ['controlled-gate', '--unchanged-scope'])
        self.assertIn(['sudo', '-n', 'systemctl', 'start', 'docker.socket', 'docker.service'], calls)
        start = next(call for call in calls if 'systemd-run' in call)
        self.assertIn('native.cgroupdriver=cgroupfs', start)
        self.assertIn('overlay2', start)
        self.assertIn('--slice', start)
        self.assertIn('--cgroup-parent', start)
        limits = next(call for call in calls if 'set-property' in call)
        self.assertEqual(limits[-2:], ['CPUQuota=200%', 'MemoryMax=4G'])
        self.assertLess(calls.index(limits), calls.index(start))
        self.assertIn(['sudo', '-n', 'mount', '-o', 'loop,pquota',
                       str(self.root / 'xfs.img'), str(self.root / 'data')], calls)

    def test_pipeline_failure_keeps_failure_and_still_reaps_engine(self):
        self.env['FAIL_STAGE'] = 'pipeline'
        done = self.execute()
        self.assertNotEqual(done.returncode, 0)
        result = json.loads((self.evidence / 'engine-result.json').read_text())
        self.assertFalse(result['success'])
        self.assertEqual(result['commandExitCode'], 1)
        self.assertEqual(result['cleanupExitCode'], 0)
        self.assertEqual(json.loads((self.state / 'pre-cleanup-evidence.json').read_text()),
                         dict(resultCopied=True, healthCopied=True, finalResultExists=False))
        self.assertFalse((self.state / 'mounted').exists())

    def test_evidence_copy_failure_still_reaps_engine_and_fails(self):
        self.env['FAIL_STAGE'] = 'copy-evidence'
        done = self.execute()
        self.assertNotEqual(done.returncode, 0)
        result = json.loads((self.evidence / 'engine-result.json').read_text())
        self.assertEqual(result['commandExitCode'], 0)
        self.assertEqual(result['cleanupExitCode'], 1)
        self.assertFalse(result['success'])
        self.assertFalse((self.state / 'mounted').exists())
        self.assertFalse((self.state / 'runtime-overrides').exists())
        self.assertTrue((self.root / 'fixture/result.json').is_file())
        self.assertIn(['sudo', '-n', 'systemctl', 'start', 'docker.socket', 'docker.service'],
                      self.calls())

    def test_limit_setup_failure_reverts_overrides_without_stopping_default_engine(self):
        self.env['FAIL_STAGE'] = 'limits'
        done = self.execute()
        self.assertNotEqual(done.returncode, 0)
        self.assertFalse((self.state / 'runtime-overrides').exists())
        self.assertFalse((self.state / 'mounted').exists())
        calls = self.calls()
        self.assertFalse(any('systemd-run' in call for call in calls))
        self.assertFalse(any('stop' in call and 'docker.service' in call for call in calls))
        self.assertEqual(json.loads((self.evidence / 'engine-result.json').read_text())[
            'commandExitCode'], 1)

    def test_mount_failure_does_not_stop_default_engine(self):
        self.env['FAIL_STAGE'] = 'mount'
        self.assertNotEqual(self.execute().returncode, 0)
        self.assertFalse(any('systemctl' in call for call in self.calls()))

    def test_failed_prestart_slice_stop_retains_limits_and_reports_cleanup_failure(self):
        self.env['FAIL_STAGE'] = 'limits-stop'
        self.assertNotEqual(self.execute().returncode, 0)
        self.assertTrue((self.state / 'runtime-overrides').exists())
        self.assertFalse((self.state / 'mounted').exists())
        calls = self.calls()
        self.assertFalse(any('revert' in call for call in calls))
        self.assertFalse(any('systemd-run' in call for call in calls))
        self.assertEqual(json.loads((self.evidence / 'engine-result.json').read_text())[
            'cleanupExitCode'], 1)

    def test_result_write_failure_fails_after_owned_cleanup(self):
        self.env['FAIL_STAGE'] = 'result-write'
        done = self.execute()
        self.assertNotEqual(done.returncode, 0)
        self.assertFalse((self.evidence / 'engine-result.json').exists())
        self.assertFalse((self.state / 'mounted').exists())
        self.assertFalse((self.state / 'runtime-overrides').exists())
        self.assertIn(['sudo', '-n', 'systemctl', 'start', 'docker.socket', 'docker.service'],
                      self.calls())

    def test_stop_failure_keeps_mounted_data_and_fails_qualification(self):
        self.env['FAIL_STAGE'] = 'stop'
        self.assertNotEqual(self.execute().returncode, 0)
        self.assertTrue((self.state / 'mounted').exists())
        self.assertTrue((self.state / 'runtime-overrides').exists())
        self.assertFalse(any('umount' in call for call in self.calls()))
        self.assertFalse(any('start' in call and 'systemctl' in call for call in self.calls()))
        self.assertTrue(any('kill' in call and 'systemctl' in call for call in self.calls()))
        self.assertEqual(json.loads((self.evidence / 'engine-result.json').read_text())[
            'cleanupExitCode'], 1)

    def test_default_inventory_drift_fails_after_engine_cleanup(self):
        self.env['FAIL_STAGE'] = 'inventory'
        self.assertNotEqual(self.execute().returncode, 0)
        self.assertFalse((self.state / 'mounted').exists())
        self.assertEqual(json.loads((self.evidence / 'engine-result.json').read_text())[
            'cleanupExitCode'], 1)

    def test_inventory_query_failure_cannot_admit_an_unknown_default_engine(self):
        self.env['FAIL_STAGE'] = 'inventory-query'
        self.assertNotEqual(self.execute().returncode, 0)
        self.assertFalse(any(call[0] == 'sudo' for call in self.calls()))
        self.assertFalse(self.root.exists())

    def test_failed_descendant_reaping_keeps_backing_filesystem_and_default_engine_stopped(self):
        self.env['FAIL_STAGE'] = 'cgroup-kill'
        self.assertNotEqual(self.execute().returncode, 0)
        self.assertTrue((self.state / 'mounted').exists())
        self.assertTrue((self.state / 'runtime-overrides').exists())
        self.assertFalse(any('umount' in call for call in self.calls()))
        self.assertFalse(any('start' in call and 'systemctl' in call for call in self.calls()))

    def test_existing_workload_and_local_invocation_fail_before_mutation(self):
        self.env['EXISTING_CONTAINER'] = '1'
        self.assertNotEqual(self.execute().returncode, 0)
        self.assertFalse(any(call[0] == 'sudo' for call in self.calls()))
        self.assertFalse(self.root.exists())
        self.env['GITHUB_ACTIONS'] = 'false'
        self.assertNotEqual(self.execute().returncode, 0)
        self.assertFalse(self.root.exists())


if __name__ == '__main__':
    unittest.main()
