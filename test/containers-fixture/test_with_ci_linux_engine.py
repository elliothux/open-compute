"""Exercise the CI Engine lifecycle with command doubles and no privileged changes."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

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
    if args[1:3] == ['systemctl', 'stop'] and args[-1].startswith('oc-ci-engine-'):
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
elif name == 'timeout':
    import subprocess
    sys.exit(subprocess.run(args[1:]).returncode)
elif name == 'findmnt':
    if not (state / 'mounted').exists(): sys.exit(1)
    print('xfs rw,prjquota')
elif name == 'docker':
    if 'container' in args:
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
    with patch('shutil.disk_usage', return_value=SimpleNamespace(free=50 << 30)):
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
                     'truncate', 'python3'):
            (self.tools / name).symlink_to(executable)
        self.root = Path('/tmp') / ('oc-ci-unit-' + self.state.name[-8:])
        self.evidence = self.state / 'evidence'
        self.env = dict(os.environ, PATH=str(self.tools) + os.pathsep + os.environ['PATH'],
                        DOUBLE_STATE=str(self.state), GITHUB_ACTIONS='true')

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

    def test_native_lifecycle_reuses_body_without_claiming_actions_identity(self):
        self.env['GITHUB_ACTIONS'] = 'false'
        done = self.execute(lifecycle=True)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertTrue(json.loads((self.evidence / 'engine-result.json').read_text())['success'])
        self.assertFalse((self.state / 'mounted').exists())
        self.assertFalse((self.state / 'runtime-overrides').exists())

    def test_success_preserves_command_and_health_and_restores_default_engine(self):
        done = self.execute()
        self.assertEqual(done.returncode, 0, done.stderr)
        result = json.loads((self.evidence / 'engine-result.json').read_text())
        self.assertTrue(result['success'])
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
        self.assertFalse((self.state / 'mounted').exists())

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
