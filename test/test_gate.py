"""Exercise scheduling and failure behavior without running product Gates."""
import importlib.util
import json
import os
import socket
from pathlib import Path
import tempfile
import threading
import time
import unittest
from unittest.mock import patch
from types import SimpleNamespace

spec = importlib.util.spec_from_file_location('gate', Path(__file__).with_name('gate.py'))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class GateTests(unittest.TestCase):
    @staticmethod
    def targets(names):
        return {name: gate.Target('package', gate.TARGETS[name][1], 'test', str(gate.ROOT),
                                  gate.TARGETS[name][2]) for name in names}

    def test_rounds_validate_before_execution(self):
        for value in ['', '0', '2', '4', '-1', '03', 'three']:
            with patch.dict(os.environ, OPEN_COMPUTE_GATE_ROUNDS=value):
                with self.assertRaises(ValueError):
                    gate.rounds_from_env()
        with patch.dict(os.environ, {}, clear=True):
            self.assertEqual(gate.rounds_from_env(), 1)
        with patch.dict(os.environ, OPEN_COMPUTE_GATE_ROUNDS='3'):
            self.assertEqual(gate.rounds_from_env(), 3)

    def test_overlapping_selections_run_each_physical_target_once(self):
        selected = gate.selection(['p0-2', 'p2-3', 'p2-5', 'p2-4', 'p1-8', 'p0-7'])
        self.assertEqual(len(selected), len(set(gate.TARGETS[name][:2] for name in selected)))
        self.assertEqual(selected.count('p0-2'), 1)
        self.assertEqual(selected.count('p2-4-product'), 1)
        with self.assertRaises(ValueError):
            gate.selection(['g0'])

    def test_parallel_targets_overlap_but_exclusive_targets_are_barriers(self):
        running = set()
        overlaps = []
        lock = threading.Lock()
        def execute(name, executable, directory, target):
            with lock:
                if gate.TARGETS[name][2]:
                    self.assertFalse(running)
                self.assertFalse(any(gate.TARGETS[item][2] for item in running))
                running.add(name)
                overlaps.append(len(running))
            time.sleep(0.02)
            with lock:
                running.remove(name)
            return {'target': name, 'exit_code': 0}
        selected = ['p0-1', 'p0-2', 'p0-3', 'runtime', 'p0-4']
        with tempfile.TemporaryDirectory() as temp:
            artifacts = {name: name for name in selected}
            results = gate.run_round(self.targets(selected), artifacts, Path(temp)/'round', 4, execute)
        self.assertEqual({result['target'] for result in results}, set(selected))
        self.assertGreater(max(overlaps), 1)

    def test_failure_stops_unscheduled_work_and_does_not_retry(self):
        calls = []
        def execute(name, executable, directory, target):
            calls.append(name)
            return {'target': name, 'exit_code': 1}
        selected = ['p0-2', 'p0-3', 'p0-4']
        with tempfile.TemporaryDirectory() as temp:
            results = gate.run_round(self.targets(selected), {n: n for n in selected},
                                     Path(temp)/'round', 1, execute)
        self.assertEqual(calls, ['p0-2'])
        self.assertEqual(len(results), 1)

    def test_top_level_repeats_only_successful_rounds_and_keeps_failure_report(self):
        for rounds, failure, prepare_failure, expected in [
            ('1', False, False, 1), ('3', False, False, 3),
            ('3', True, False, 1), ('3', True, True, 0),
        ]:
            with tempfile.TemporaryDirectory() as temp, \
                 patch.object(gate, 'ROOT', Path(temp)), \
                 patch.object(gate, 'verify_inputs', return_value={}), \
                 patch.object(gate, 'source_identity', return_value='unchanged'), \
                 patch.object(gate, 'resolve_targets', return_value=self.targets(['p0-2'])), \
                 patch.object(gate, 'build_targets', return_value=({}, {'invocations': 1})), \
                 patch.object(gate.platform, 'platform', return_value='test-host'), \
                 patch.object(gate.subprocess, 'check_output', return_value='revision'), \
                 patch.object(gate.sys, 'argv', ['gate.py', 'p0-2']), \
                 patch.dict(os.environ, OPEN_COMPUTE_GATE_ROUNDS=rounds), \
                 patch.object(gate, 'run_round', side_effect=[
                     [{'target': 'p0-2', 'exit_code': int(prepare_failure)}],
                     *[[{'target': 'p0-2', 'exit_code': int(failure)}]] * expected,
                 ]) as run:
                self.assertEqual(gate.main(), int(failure))
                self.assertEqual(run.call_count, expected + 1)
                reports = list(Path(temp).rglob('report.json'))
                self.assertEqual(len(reports), 1)
                self.assertEqual('failed' in reports[0].parts, failure)
                report = json.loads(reports[0].read_text())
                self.assertEqual(report['preparation']['processes_executed'], 1)
                self.assertEqual(report['test_processes_executed'], expected)

    def test_harness_preparation_only_lists_tests_in_a_separate_process(self):
        with tempfile.TemporaryDirectory() as temp, \
             patch.object(gate.subprocess, 'run', return_value=SimpleNamespace(returncode=0)) as run, \
             patch.dict(os.environ, OPEN_COMPUTE_GATE_ROUNDS='3'):
            target = self.targets(['p0-2'])['p0-2']
            result = gate.execute_target('p0-2', '/compiled/test', Path(temp)/'prepare', target,
                                         list_only=True)
            self.assertEqual(result['exit_code'], 0)
            self.assertEqual(run.call_args.args[0], ['/compiled/test', '--list'])
            self.assertEqual(run.call_args.kwargs['timeout'], 600)
            self.assertNotIn('OPEN_COMPUTE_GATE_ROUNDS', run.call_args.kwargs['env'])

    def test_long_report_paths_do_not_break_unix_sockets_or_discard_leftovers(self):
        with tempfile.TemporaryDirectory(dir='/tmp') as temp, \
             patch.object(gate, 'ROOT', Path(temp)):
            parent = Path(temp) / ('long-report-name-' * 8)
            parent.mkdir()
            target = self.targets(['p0-2'])['p0-2']
            def execute(command, **kwargs):
                scratch = Path(kwargs['env']['TMPDIR'])
                with tempfile.TemporaryDirectory(dir=scratch) as nested, \
                     socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
                    listener.bind(str(Path(nested)/'socket'))
                return SimpleNamespace(returncode=0)
            with patch.object(gate.subprocess, 'run', side_effect=execute):
                result = gate.execute_target('p0-2', '/compiled/test', parent/'pass', target)
            self.assertEqual(result['exit_code'], 0)
            self.assertEqual(list((Path(temp)/'.temp/gate-tmp').iterdir()), [])
            def leak(command, **kwargs):
                (Path(kwargs['env']['TMPDIR'])/'evidence').write_text('preserve')
                return SimpleNamespace(returncode=0)
            with patch.object(gate.subprocess, 'run', side_effect=leak):
                result = gate.execute_target('p0-2', '/compiled/test', parent/'leak', target)
            self.assertEqual(result['exit_code'], 1)
            self.assertEqual((parent/'leak/tmp/evidence').read_text(), 'preserve')

    def test_supervisor_relative_diagnostics_stay_in_the_run_directory(self):
        with tempfile.TemporaryDirectory() as temp, \
             patch.object(gate, 'ROOT', Path(temp)):
            directory = Path(temp)/'runtime'
            target = self.targets(['runtime'])['runtime']
            def execute(command, **kwargs):
                # The fixture can write relative output even after env_clear().
                (Path(kwargs['cwd'])/'fixture.diagnostic').write_text('retained')
                return SimpleNamespace(returncode=0)
            with patch.object(gate.subprocess, 'run', side_effect=execute):
                result = gate.execute_target('runtime', '/compiled/test', directory, target)
            self.assertEqual(result['exit_code'], 0)
            self.assertEqual(Path(result['cwd']), directory.resolve())
            self.assertEqual((directory/'fixture.diagnostic').read_text(), 'retained')

    def test_workspace_keeps_every_cargo_target_and_unknown_targets_exclusive(self):
        metadata = {'workspace_members': ['service', 'runtime'], 'packages': [{
            'id': 'service', 'name': 'open-compute-service', 'manifest_path': '/repo/crates/service/Cargo.toml',
            'targets': [
                {'name': 'cli', 'kind': ['test'], 'test': True},
                {'name': 'open_compute_service', 'kind': ['lib'], 'test': True},
                {'name': 'p0_2_runtime_gate', 'kind': ['test'], 'test': True},
                {'name': 'new_test', 'kind': ['test'], 'test': True},
                {'name': 'platformd', 'kind': ['bin'], 'test': True},
                {'name': 'build-script-build', 'kind': ['custom-build'], 'test': False},
            ],
        }, {'id': 'runtime', 'name': 'open-compute-runtime',
            'manifest_path': '/repo/crates/runtime/Cargo.toml',
            'targets': [{'name': 'open_compute_runtime', 'kind': ['lib'], 'test': True}]}]}
        with patch.object(gate.subprocess, 'check_output', return_value=json.dumps(metadata)):
            targets = gate.resolve_targets([], True)
        self.assertEqual({target.name for target in targets.values()},
                         {'cli', 'open_compute_service', 'open_compute_runtime',
                          'p0_2_runtime_gate', 'new_test', 'platformd'})
        self.assertEqual(next(iter(targets)), 'open-compute-service.test.cli')
        self.assertTrue(targets['open-compute-service.test.cli'].exclusive)
        self.assertFalse(targets['open-compute-service.lib.open_compute_service'].exclusive)
        self.assertTrue(targets['open-compute-runtime.lib.open_compute_runtime'].exclusive)
        self.assertFalse(targets['p0-2'].exclusive)
        self.assertTrue(targets['open-compute-service.test.new_test'].exclusive)
        self.assertTrue(all(target.cwd == '/repo/crates/service'
                            for target in targets.values() if target.package_id == 'service'))

    def test_workspace_rejects_repetition_before_build(self):
        with patch.object(gate.sys, 'argv', ['gate.py', '--workspace']), \
             patch.dict(os.environ, OPEN_COMPUTE_GATE_ROUNDS='3'), \
             patch.object(gate, 'resolve_targets') as resolve:
            with self.assertRaisesRegex(ValueError, 'one round'):
                gate.main()
            resolve.assert_not_called()

    def test_source_freeze_ignores_designs_but_includes_code_and_consumed_references(self):
        names = ['crates/service/src/resources.rs', 'docs/references/runbooks/install.md',
                 'docs/plan.md', 'docs/implemented/report.md']
        with tempfile.TemporaryDirectory() as temp, \
             patch.object(gate, 'ROOT', Path(temp)), \
             patch.object(gate.subprocess, 'check_output', return_value='\0'.join(names).encode()):
            for name in names:
                path = Path(temp)/name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('original')
            baseline = gate.source_identity()
            for name in names[2:]:
                (Path(temp)/name).write_text('unrelated documentation update')
            self.assertEqual(gate.source_identity(), baseline)
            for name in names[:2]:
                (Path(temp)/name).write_text('changed runtime input')
                self.assertNotEqual(gate.source_identity(), baseline)
                (Path(temp)/name).write_text('original')

    def test_build_matches_package_and_kind_and_rejects_missing_executables(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            binary = root / 'test-binary'
            binary.write_bytes(b'compiled test')
            targets = {'first': gate.Target('one', 'same', 'lib', temp, False),
                       'second': gate.Target('two', 'same', 'test', temp, False)}
            messages = [{'reason': 'compiler-artifact', 'package_id': target.package_id,
                         'target': {'name': target.name, 'kind': [target.kind]},
                         'profile': {'test': True}, 'executable': str(binary)}
                        for target in targets.values()]
            result = SimpleNamespace(returncode=0, stdout='\n'.join(map(json.dumps, messages)))
            with patch.object(gate.subprocess, 'run', return_value=result) as run:
                artifacts, build = gate.build_targets(targets, root, True)
            self.assertEqual(set(artifacts), set(targets))
            self.assertEqual(build['invocations'], 1)
            self.assertIn('--all-targets', run.call_args.args[0])
            missing = root / 'missing'
            missing.mkdir()
            result.stdout = json.dumps(messages[0])
            with patch.object(gate.subprocess, 'run', return_value=result):
                with self.assertRaisesRegex(RuntimeError, 'did not produce'):
                    gate.build_targets(targets, missing, True)


if __name__ == '__main__':
    unittest.main()
