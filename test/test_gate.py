"""Exercise scheduling and failure behavior without running product Gates."""
import importlib.util
import json
import os
import socket
import shutil
import subprocess
import sys
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch
from types import SimpleNamespace

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location('gate', Path(__file__).with_name('gate.py'))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class GateTests(unittest.TestCase):
    def test_python_main_is_one_exclusive_registered_scenario_in_acceptance_groups(self):
        name = 'p21-python-main'
        case = 'python_main::p21_python_main_upload_prepare_dispatch_restart_rollback'
        self.assertEqual(gate.CARGO_TARGETS[name], ('open-compute-service', 'p21_python_main', True))
        self.assertEqual(gate.TIMING[name], (case,))
        self.assertFalse(gate.ONCE.get(name, ()))
        self.assertEqual(gate.GROUPS['all'].count(name), 1)
        self.assertEqual(gate.P3_PRODUCT_TARGETS.count(name), 1)
        metadata = {'workspace_members': ['service'], 'packages': [{
            'id': 'service', 'name': 'open-compute-service', 'manifest_path': '/repo/crates/service/Cargo.toml',
            'targets': [{'name': 'p21_python_main', 'kind': ['test'], 'test': True}],
        }]}
        with patch.object(gate.subprocess, 'check_output', return_value=json.dumps(metadata)), \
             patch.object(gate, 'CARGO_TARGETS', {name: gate.CARGO_TARGETS[name]}):
            targets = gate.resolve_targets([name], False)
            workspace = gate.resolve_targets([], True)
        self.assertEqual(targets[name].name, 'p21_python_main')
        self.assertTrue(targets[name].exclusive)
        self.assertEqual(targets[name].cases, (case,))
        self.assertIn(name, workspace)
        self.assertEqual(workspace[name].name, 'p21_python_main')

    @staticmethod
    def targets(names):
        return {name: gate.Target('package', gate.TARGETS[name][1], 'test', str(gate.ROOT),
                                  gate.TARGETS[name][2]) for name in names}

    def test_python_frameworks_require_three_cases_in_one_exclusive_native_inventory(self):
        name = 'p21-python-frameworks'
        cases = tuple(sorted(
            f'frameworks::p21_python_{framework}_framework_deploy_restart_rollback'
            for framework in ('django', 'flask', 'fastapi')
        ))
        self.assertEqual(gate.CARGO_TARGETS[name], ('open-compute-service', 'p21_python_frameworks', True))
        self.assertEqual(tuple(sorted(gate.TIMING[name])), cases)
        self.assertFalse(gate.ONCE.get(name, ()))
        self.assertEqual(gate.GROUPS['all'].count(name), 1)
        self.assertEqual(gate.P3_PRODUCT_TARGETS.count(name), 1)
        catalog = json.loads((gate.ROOT / 'test/conformance/catalog.json').read_text())
        references = {f'{name}::{case}' for case in cases}
        for contract in catalog['contracts']:
            for polarity in ('positiveCases', 'negativeCases'):
                owned = references.intersection(contract[polarity])
                expected = references if contract['id'] in (
                    'workers.runtime.common', 'deployments.immutable.lifecycle',
                ) else set()
                self.assertEqual(owned, expected, contract['id'])
        gate.validate_contract_case_mapping()
        metadata = {'workspace_members': ['service'], 'packages': [{
            'id': 'service', 'name': 'open-compute-service', 'manifest_path': '/repo/crates/service/Cargo.toml',
            'targets': [{'name': 'p21_python_frameworks', 'kind': ['test'], 'test': True}],
        }]}
        with patch.object(gate.subprocess, 'check_output', return_value=json.dumps(metadata)), \
             patch.object(gate, 'CARGO_TARGETS', {name: gate.CARGO_TARGETS[name]}):
            targets = gate.resolve_targets([name], False)
            workspace = gate.resolve_targets([], True)
        self.assertEqual(targets[name].cases, cases)
        self.assertTrue(targets[name].exclusive)
        self.assertIn(name, workspace)
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / 'list.log'
            log.write_text('\n'.join(f'{case}: test' for case in cases) + '\n\n3 tests, 0 benchmarks\n')
            gate.verify_case_inventory(targets, [{'target': name, 'log': str(log)}])
            log.write_text(f'{cases[0]}: test\n\n1 test, 0 benchmarks\n')
            with self.assertRaisesRegex(ValueError, 'case registry mismatch'):
                gate.verify_case_inventory(targets, [{'target': name, 'log': str(log)}])

    def test_python_services_owns_native_case_and_matching_contract_evidence(self):
        name = 'p21-python-services'
        case = 'services::p21_python_services_fetch_named_rpc_callback_restart_rollback'
        self.assertEqual(gate.CARGO_TARGETS[name], ('open-compute-service', 'p21_python_services', True))
        self.assertEqual(gate.TIMING[name], (case,))
        self.assertFalse(gate.ONCE.get(name, ()))
        self.assertEqual(gate.GROUPS['all'].count(name), 1)
        self.assertEqual(gate.P3_PRODUCT_TARGETS.count(name), 1)
        metadata = {'workspace_members': ['service'], 'packages': [{
            'id': 'service', 'name': 'open-compute-service', 'manifest_path': '/repo/crates/service/Cargo.toml',
            'targets': [{'name': 'p21_python_services', 'kind': ['test'], 'test': True}],
        }]}
        with patch.object(gate.subprocess, 'check_output', return_value=json.dumps(metadata)), \
             patch.object(gate, 'CARGO_TARGETS', {name: gate.CARGO_TARGETS[name]}):
            targets = gate.resolve_targets([name], False)
            workspace = gate.resolve_targets([], True)
        self.assertEqual(targets[name].cases, (case,))
        self.assertTrue(targets[name].exclusive)
        self.assertIn(name, workspace)
        catalog = json.loads((gate.ROOT / 'test/conformance/catalog.json').read_text())
        for contract in catalog['contracts']:
            owned = f'{name}::{case}'
            expected = contract['id'] in (
                'workers.runtime.common', 'deployments.immutable.lifecycle', 'services.fetch.rpc',
            )
            for polarity in ('positiveCases', 'negativeCases'):
                self.assertEqual(owned in contract[polarity], expected, contract['id'])
        gate.validate_contract_case_mapping()
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / 'list.log'
            log.write_text(f'{case}: test\n\n1 test, 0 benchmarks\n')
            gate.verify_case_inventory(targets, [{'target': name, 'log': str(log)}])
            log.write_text('0 tests, 0 benchmarks\n')
            with self.assertRaisesRegex(ValueError, 'case registry mismatch'):
                gate.verify_case_inventory(targets, [{'target': name, 'log': str(log)}])

    def test_python_queues_owns_native_case_and_matching_contract_evidence(self):
        name = 'p21-python-queues'
        case = 'queues::p21_python_queues_produce_consume_retry_dlq_restart_rollback'
        self.assertEqual(gate.CARGO_TARGETS[name], ('open-compute-service', 'p21_python_queues', True))
        self.assertEqual(gate.TIMING[name], (case,))
        self.assertFalse(gate.ONCE.get(name, ()))
        self.assertEqual(gate.GROUPS['all'].count(name), 1)
        self.assertEqual(gate.P3_PRODUCT_TARGETS.count(name), 1)
        metadata = {'workspace_members': ['service'], 'packages': [{
            'id': 'service', 'name': 'open-compute-service', 'manifest_path': '/repo/crates/service/Cargo.toml',
            'targets': [{'name': 'p21_python_queues', 'kind': ['test'], 'test': True}],
        }]}
        with patch.object(gate.subprocess, 'check_output', return_value=json.dumps(metadata)), \
             patch.object(gate, 'CARGO_TARGETS', {name: gate.CARGO_TARGETS[name]}):
            targets = gate.resolve_targets([name], False)
            workspace = gate.resolve_targets([], True)
        self.assertEqual(targets[name].cases, (case,))
        self.assertTrue(targets[name].exclusive)
        self.assertIn(name, workspace)
        catalog = json.loads((gate.ROOT / 'test/conformance/catalog.json').read_text())
        for contract in catalog['contracts']:
            owned = f'{name}::{case}'
            expected = contract['id'] in (
                'workers.runtime.common', 'deployments.immutable.lifecycle', 'queues.push.methods',
            )
            for polarity in ('positiveCases', 'negativeCases'):
                self.assertEqual(owned in contract[polarity], expected, contract['id'])
        gate.validate_contract_case_mapping()
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / 'list.log'
            log.write_text(f'{case}: test\n\n1 test, 0 benchmarks\n')
            gate.verify_case_inventory(targets, [{'target': name, 'log': str(log)}])
            log.write_text('0 tests, 0 benchmarks\n')
            with self.assertRaisesRegex(ValueError, 'case registry mismatch'):
                gate.verify_case_inventory(targets, [{'target': name, 'log': str(log)}])

    def test_python_workflows_owns_native_case_and_matching_contract_evidence(self):
        name = 'p21-python-workflows'
        case = 'workflows::p21_python_workflows_steps_events_retry_pause_restart_rollback'
        self.assertEqual(gate.CARGO_TARGETS[name], ('open-compute-service', 'p21_python_workflows', True))
        self.assertEqual(gate.TIMING[name], (case,))
        self.assertFalse(gate.ONCE.get(name, ()))
        self.assertEqual(gate.GROUPS['all'].count(name), 1)
        self.assertEqual(gate.P3_PRODUCT_TARGETS.count(name), 1)
        metadata = {'workspace_members': ['service'], 'packages': [{
            'id': 'service', 'name': 'open-compute-service', 'manifest_path': '/repo/crates/service/Cargo.toml',
            'targets': [{'name': 'p21_python_workflows', 'kind': ['test'], 'test': True}],
        }]}
        with patch.object(gate.subprocess, 'check_output', return_value=json.dumps(metadata)), \
             patch.object(gate, 'CARGO_TARGETS', {name: gate.CARGO_TARGETS[name]}):
            targets = gate.resolve_targets([name], False)
            workspace = gate.resolve_targets([], True)
        self.assertEqual(targets[name].cases, (case,))
        self.assertTrue(targets[name].exclusive)
        self.assertIn(name, workspace)
        catalog = json.loads((gate.ROOT / 'test/conformance/catalog.json').read_text())
        for contract in catalog['contracts']:
            owned = f'{name}::{case}'
            expected = contract['id'] in (
                'workers.runtime.common', 'deployments.immutable.lifecycle', 'workflows.binding.lifecycle',
            )
            for polarity in ('positiveCases', 'negativeCases'):
                self.assertEqual(owned in contract[polarity], expected, contract['id'])
        gate.validate_contract_case_mapping()
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / 'list.log'
            log.write_text(f'{case}: test\n\n1 test, 0 benchmarks\n')
            gate.verify_case_inventory(targets, [{'target': name, 'log': str(log)}])
            log.write_text('0 tests, 0 benchmarks\n')
            with self.assertRaisesRegex(ValueError, 'case registry mismatch'):
                gate.verify_case_inventory(targets, [{'target': name, 'log': str(log)}])

    def test_python_runtime_owns_native_case_and_matching_contract_evidence(self):
        name = 'p21-python-runtime'
        case = 'runtime::p21_python_runtime_ffi_stdlib_wait_until_network_restart_rollback'
        self.assertEqual(gate.CARGO_TARGETS[name], ('open-compute-service', 'p21_python_runtime', True))
        self.assertEqual(gate.TIMING[name], (case,))
        self.assertFalse(gate.ONCE.get(name, ()))
        self.assertEqual(gate.GROUPS['all'].count(name), 1)
        self.assertEqual(gate.P3_PRODUCT_TARGETS.count(name), 1)
        metadata = {'workspace_members': ['service'], 'packages': [{
            'id': 'service', 'name': 'open-compute-service', 'manifest_path': '/repo/crates/service/Cargo.toml',
            'targets': [{'name': 'p21_python_runtime', 'kind': ['test'], 'test': True}],
        }]}
        with patch.object(gate.subprocess, 'check_output', return_value=json.dumps(metadata)), \
             patch.object(gate, 'CARGO_TARGETS', {name: gate.CARGO_TARGETS[name]}):
            targets = gate.resolve_targets([name], False)
            workspace = gate.resolve_targets([], True)
        self.assertEqual(targets[name].cases, (case,))
        self.assertTrue(targets[name].exclusive)
        self.assertIn(name, workspace)
        catalog = json.loads((gate.ROOT / 'test/conformance/catalog.json').read_text())
        for contract in catalog['contracts']:
            owned = f'{name}::{case}'
            expected = contract['id'] in (
                'workers.runtime.common', 'deployments.immutable.lifecycle', 'kv.namespace.methods', 'd1.binding.methods', 'r2.bucket.methods',
            )
            for polarity in ('positiveCases', 'negativeCases'):
                self.assertEqual(owned in contract[polarity], expected, contract['id'])
        gate.validate_contract_case_mapping()
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / 'list.log'
            log.write_text(f'{case}: test\n\n1 test, 0 benchmarks\n')
            gate.verify_case_inventory(targets, [{'target': name, 'log': str(log)}])
            log.write_text('0 tests, 0 benchmarks\n')
            with self.assertRaisesRegex(ValueError, 'case registry mismatch'):
                gate.verify_case_inventory(targets, [{'target': name, 'log': str(log)}])

    def test_python_durable_objects_owns_native_case_and_matching_contract_evidence(self):
        name = 'p21-python-durable-objects'
        case = 'durable_objects::p21_python_durable_objects_fetch_rpc_storage_alarm_restart_rollback'
        self.assertEqual(gate.CARGO_TARGETS[name], ('open-compute-service', 'p21_python_durable_objects', True))
        self.assertEqual(gate.TIMING[name], (case,))
        self.assertFalse(gate.ONCE.get(name, ()))
        self.assertEqual(gate.GROUPS['all'].count(name), 1)
        self.assertEqual(gate.P3_PRODUCT_TARGETS.count(name), 1)
        metadata = {'workspace_members': ['service'], 'packages': [{
            'id': 'service', 'name': 'open-compute-service', 'manifest_path': '/repo/crates/service/Cargo.toml',
            'targets': [{'name': 'p21_python_durable_objects', 'kind': ['test'], 'test': True}],
        }]}
        with patch.object(gate.subprocess, 'check_output', return_value=json.dumps(metadata)), \
             patch.object(gate, 'CARGO_TARGETS', {name: gate.CARGO_TARGETS[name]}):
            targets = gate.resolve_targets([name], False)
            workspace = gate.resolve_targets([], True)
        self.assertEqual(targets[name].cases, (case,))
        self.assertTrue(targets[name].exclusive)
        self.assertIn(name, workspace)
        catalog = json.loads((gate.ROOT / 'test/conformance/catalog.json').read_text())
        for contract in catalog['contracts']:
            owned = f'{name}::{case}'
            expected = contract['id'] in (
                'workers.runtime.common', 'deployments.immutable.lifecycle', 'durable-objects.namespace.storage', 'durable-objects.alarms',
            )
            for polarity in ('positiveCases', 'negativeCases'):
                self.assertEqual(owned in contract[polarity], expected, contract['id'])
        gate.validate_contract_case_mapping()
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / 'list.log'
            log.write_text(f'{case}: test\n\n1 test, 0 benchmarks\n')
            gate.verify_case_inventory(targets, [{'target': name, 'log': str(log)}])
            log.write_text('0 tests, 0 benchmarks\n')
            with self.assertRaisesRegex(ValueError, 'case registry mismatch'):
                gate.verify_case_inventory(targets, [{'target': name, 'log': str(log)}])

    def test_runtime_inputs_use_only_the_pinned_bundled_archive_or_explicit_copy(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            runtime = root / 'packages/runtime'
            (runtime / 'dist').mkdir(parents=True)
            (runtime / 'dist/manifest.json').write_text('{}')
            binary = root / 'workerd'
            binary.write_bytes(b'binary fixture')
            archive = root / 'explicit.gz'
            archive.write_bytes(b'archive fixture')
            arch = {'aarch64': 'arm64', 'arm64': 'arm64', 'x86_64': 'x64', 'AMD64': 'x64'}[gate.platform.machine()]
            target = f'{sys.platform}-{arch}'
            entry = {'binarySha256': gate.digest(binary), 'archiveSha256': gate.digest(archive),
                     'archiveName': 'workerd.gz'}
            (runtime / 'workerd.lock.json').write_text(json.dumps({
                'release': 'fixture', 'targets': {target: entry},
            }))
            bundled = root / '.temp/workerd-build' / target / entry['archiveSha256'] / entry['archiveName']
            bundled.parent.mkdir(parents=True)
            bundled.write_bytes(archive.read_bytes())
            with patch.object(gate, 'ROOT', root), patch.dict(os.environ, {
                'OPEN_COMPUTE_TEST_WORKERD': str(binary),
            }, clear=True):
                result = gate.verify_inputs(probe_version=False)
                self.assertEqual(result['archiveSha256'], entry['archiveSha256'])
                with patch.dict(os.environ, OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE=str(archive)):
                    self.assertEqual(gate.verify_inputs(probe_version=False), result)
                with patch.dict(os.environ, OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE=str(root / 'missing')):
                    with self.assertRaisesRegex(ValueError, 'existing absolute regular file'):
                        gate.verify_inputs(probe_version=False)
                bundled.write_bytes(b'corrupt')
                with self.assertRaisesRegex(ValueError, 'SHA-256'):
                    gate.verify_inputs(probe_version=False)
                bundled.unlink()
                bundled.symlink_to(archive)
                with self.assertRaisesRegex(ValueError, 'regular file'):
                    gate.verify_inputs(probe_version=False)
                bundled.unlink()
                with self.assertRaisesRegex(ValueError, 'existing absolute regular file'):
                    gate.verify_inputs(probe_version=False)

    def test_rounds_validate_before_execution(self):
        for value in ['', '0', '2', '4', '-1', '03', 'three']:
            with patch.dict(os.environ, OPEN_COMPUTE_GATE_ROUNDS=value):
                with self.assertRaises(ValueError):
                    gate.rounds_from_env()
        with patch.dict(os.environ, {}, clear=True):
            self.assertEqual(gate.rounds_from_env(), 1)
        with patch.dict(os.environ, OPEN_COMPUTE_GATE_ROUNDS='3'):
            self.assertEqual(gate.rounds_from_env(), 3)

    def test_macos_gate_reexecs_under_caffeinate_once(self):
        with patch.object(gate.sys, 'platform', 'darwin'), \
             patch.dict(os.environ, {}, clear=True), \
             patch.object(gate.os, 'execve', side_effect=RuntimeError('exec')) as execute:
            with self.assertRaisesRegex(RuntimeError, 'exec'):
                gate.prevent_macos_sleep()
            self.assertEqual(
                execute.call_args.args[:2],
                ('/usr/bin/caffeinate', [
                    '/usr/bin/caffeinate', '-is', sys.executable,
                    str(Path(gate.__file__).resolve()), *sys.argv[1:],
                ]),
            )
            self.assertEqual(
                execute.call_args.args[2]['OPEN_COMPUTE_CAFFEINATED'], '1')
        with patch.object(gate.sys, 'platform', 'darwin'), \
             patch.dict(os.environ, {'OPEN_COMPUTE_CAFFEINATED': '1'}, clear=True), \
             patch.object(gate.os, 'execve') as execute:
            gate.prevent_macos_sleep()
            execute.assert_not_called()
            self.assertNotIn('OPEN_COMPUTE_CAFFEINATED', os.environ)
        with patch.object(gate.sys, 'platform', 'darwin'), \
             patch.dict(os.environ, {'GITHUB_ACTIONS': 'true'}, clear=True), \
             patch.object(gate.os, 'execve') as execute:
            gate.prevent_macos_sleep()
            execute.assert_not_called()

    def test_overlapping_selections_run_each_physical_target_once(self):
        selected = gate.selection(['p0-2', 'p2-3', 'workflow', 'p1-8', 'p0-7'])
        self.assertEqual(len(selected), len(set(gate.TARGETS[name][:2] for name in selected)))
        self.assertEqual(selected.count('p0-2'), 1)
        self.assertEqual(selected.count('workflow-product'), 1)
        with self.assertRaises(ValueError):
            gate.selection(['g0'])

    def test_parallel_targets_overlap_but_exclusive_targets_are_barriers(self):
        running = set()
        completed = set()
        overlaps = []
        lock = threading.Lock()
        parallel_started = threading.Barrier(2)
        def execute(name, executable, directory, target):
            with lock:
                if target.exclusive:
                    self.assertFalse(running)
                self.assertFalse(any(targets[item].exclusive for item in running))
                if name == 'service-process-lifecycle':
                    self.assertEqual(completed, {'service-lib', 'peer'})
                if name == 'after':
                    self.assertIn('service-process-lifecycle', completed)
                running.add(name)
                overlaps.append(len(running))
            if name in {'service-lib', 'peer'}:
                parallel_started.wait(timeout=5)
            with lock:
                running.remove(name)
                completed.add(name)
            return {'target': name, 'exit_code': 0}
        targets = {
            'service-lib': gate.Target('service', 'lib', 'lib', '/repo', False),
            'peer': gate.Target('peer', 'lib', 'lib', '/repo', False),
            'service-process-lifecycle': gate.Target('service', 'lib', 'lib', '/repo', True),
            'after': gate.Target('after', 'lib', 'lib', '/repo', False),
        }
        with tempfile.TemporaryDirectory() as temp:
            artifacts = {name: name for name in targets}
            results = gate.run_round(targets, artifacts, Path(temp)/'round', 2, execute)
        self.assertEqual({result['target'] for result in results}, set(targets))
        self.assertTrue(all(result['exit_code'] == 0 for result in results), results)
        self.assertEqual(completed, set(targets))
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

    def test_keep_going_collects_failures_without_retrying_targets(self):
        calls = []
        selected = ['p0-2', 'p0-3', 'p0-4']
        def execute(name, executable, directory, target):
            calls.append(name)
            return {'target': name, 'exit_code': int(name != 'p0-3')}
        with tempfile.TemporaryDirectory() as temp:
            results = gate.run_round(self.targets(selected), {n: n for n in selected},
                                     Path(temp)/'round', 1, execute, keep_going=True)
        self.assertEqual(calls, selected)
        self.assertEqual([item['exit_code'] for item in results], [1, 0, 1])

    def test_workspace_execution_requires_explicit_final_phase(self):
        with patch.object(gate.sys, 'argv', ['gate.py', '--workspace']), \
             patch.dict(os.environ, {}, clear=True), \
             patch.object(gate, 'resolve_targets') as resolve:
            with self.assertRaisesRegex(ValueError, 'requires --final'):
                gate.main()
            resolve.assert_not_called()

    def test_final_rejects_instrumented_or_repeated_execution(self):
        for environment in [{'OPEN_COMPUTE_GATE_ROUNDS': '3'},
                            {'RUSTFLAGS': '-C instrument-coverage'}]:
            with patch.object(gate.sys, 'argv', ['gate.py', '--workspace', '--final']), \
                 patch.dict(os.environ, environment, clear=True), \
                 patch.object(gate, 'resolve_targets') as resolve:
                with self.assertRaisesRegex(ValueError, 'one uninstrumented round'):
                    gate.main()
                resolve.assert_not_called()

    def test_duplicate_final_rejects_passed_and_failed_frozen_inputs(self):
        for failed in [False, True]:
            with tempfile.TemporaryDirectory() as temp, patch.object(gate, 'ROOT', Path(temp)):
                directory = Path(temp)/'.temp/gate-run'
                directory = directory/'failed/run' if failed else directory/'run'
                directory.mkdir(parents=True)
                (directory/'report.json').write_text(json.dumps({
                    'purpose': 'final', 'workspace': True, 'source_sha256': 'frozen',
                    'inputs': {'runtime': 'verified'}, 'status': 'failed' if failed else 'passed',
                }))
                with self.assertRaisesRegex(ValueError, 'already attempted'):
                    gate.reject_duplicate_final('frozen', {'runtime': 'verified'})
                gate.reject_duplicate_final('changed', {'runtime': 'verified'})
                gate.reject_duplicate_final('frozen', {'runtime': 'different'})

    def test_top_level_repeats_only_successful_rounds_and_keeps_failure_report(self):
        for rounds, failure, prepare_failure, expected in [
            ('1', False, False, 1), ('3', False, False, 3),
            ('3', True, False, 1), ('3', True, True, 0),
        ]:
            with tempfile.TemporaryDirectory() as temp, \
                 patch.object(gate, 'ROOT', Path(temp)), \
                 patch.object(gate, 'verify_inputs', return_value={}), \
                 patch.object(gate, 'validate_contract_case_mapping'), \
                 patch.object(gate, 'write_contract_report'), \
                 patch.object(gate, 'source_identity', return_value='unchanged'), \
                 patch.object(gate, 'resolve_targets', return_value=self.targets(['p0-2'])), \
                 patch.object(gate, 'build_targets', return_value=({}, {'invocations': 1})), \
                 patch.object(gate, 'verify_case_inventory', return_value={
                     name: target._replace(cases=gate.TIMING[name])
                     for name, target in self.targets(['p0-2']).items()}), \
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
             patch.dict(os.environ, OPEN_COMPUTE_GATE_ROUNDS='3',
                        BUN_RUNTIME_TRANSPILER_CACHE_PATH='unowned-cache',
                        NODE_DISABLE_COMPILE_CACHE='0'):
            target = self.targets(['p0-2'])['p0-2']
            result = gate.execute_target('p0-2', '/compiled/test', Path(temp)/'prepare', target,
                                         list_only=True)
            self.assertEqual(result['exit_code'], 0)
            self.assertEqual(run.call_args.args[0], ['/compiled/test', '--list'])
            self.assertEqual(run.call_args.kwargs['timeout'], 600)
            self.assertNotIn('OPEN_COMPUTE_GATE_ROUNDS', run.call_args.kwargs['env'])
            self.assertEqual(run.call_args.kwargs['env']['BUN_RUNTIME_TRANSPILER_CACHE_PATH'], '0')
            self.assertEqual(run.call_args.kwargs['env']['NODE_DISABLE_COMPILE_CACHE'], '1')

    def test_p5_search_defaults_fixture_embedding_key(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(gate, 'ROOT', Path(temp)):
            target = self.targets(['p5-search'])['p5-search']
            captured = {}
            def execute(command, **kwargs):
                captured['env'] = kwargs['env']
                return SimpleNamespace(returncode=0)
            with patch.dict(os.environ, {'PATH': '/usr/bin'}, clear=True), \
                 patch.object(gate.subprocess, 'run', side_effect=execute):
                result = gate.execute_target(
                    'p5-search', '/compiled/test', Path(temp)/'default', target)
            self.assertEqual(result['exit_code'], 0)
            self.assertEqual(
                captured['env']['OPEN_COMPUTE_TEST_EMBEDDING_API_KEY'], 'fixture-secret')
            captured.clear()
            with patch.dict(os.environ, {
                    'PATH': '/usr/bin',
                    'OPEN_COMPUTE_TEST_EMBEDDING_API_KEY': 'operator-key',
            }), patch.object(gate.subprocess, 'run', side_effect=execute):
                result = gate.execute_target(
                    'p5-search', '/compiled/test', Path(temp)/'operator', target)
            self.assertEqual(result['exit_code'], 0)
            self.assertEqual(
                captured['env']['OPEN_COMPUTE_TEST_EMBEDDING_API_KEY'], 'operator-key')
            captured.clear()
            with patch.dict(os.environ, {'PATH': '/usr/bin'}, clear=True), \
                 patch.object(gate.subprocess, 'run', side_effect=execute):
                result = gate.execute_target(
                    'p0-2', '/compiled/test', Path(temp)/'other', self.targets(['p0-2'])['p0-2'])
            self.assertEqual(result['exit_code'], 0)
            self.assertNotIn('OPEN_COMPUTE_TEST_EMBEDDING_API_KEY', captured['env'])

    def test_cli_policy_import_does_not_create_source_tree_bytecode(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for name in ['gate.py', 'gate_cases.py']:
                (root/name).write_bytes(Path(__file__).with_name(name).read_bytes())
            env = dict(os.environ)
            env.pop('PYTHONDONTWRITEBYTECODE', None)
            env.pop('PYTHONPYCACHEPREFIX', None)
            result = subprocess.run([sys.executable, str(root/'gate.py'), '--help'],
                                    env=env, capture_output=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            self.assertEqual({p.name for p in root.iterdir()}, {'gate.py', 'gate_cases.py'})

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
                {'name': 'p5_search_gate', 'kind': ['test'], 'test': True},
                {'name': 'single_binary', 'kind': ['test'], 'test': True},
                {'name': 'new_test', 'kind': ['test'], 'test': True},
                {'name': 'ocd', 'kind': ['bin'], 'test': True},
                {'name': 'build-script-build', 'kind': ['custom-build'], 'test': False},
            ],
        }, {'id': 'runtime', 'name': 'open-compute-runtime',
            'manifest_path': '/repo/crates/runtime/Cargo.toml',
            'targets': [{'name': 'open_compute_runtime', 'kind': ['lib'], 'test': True}]}]}
        with patch.object(gate.subprocess, 'check_output', return_value=json.dumps(metadata)), \
             patch.object(gate, 'CARGO_TARGETS', {
                 name: gate.CARGO_TARGETS[name] for name in ('p0-2', 'p5-search', 'single-binary')
             }):
            targets = gate.resolve_targets([], True)
        self.assertEqual({target.name for target in targets.values()},
                          {'cli', 'open_compute_service', 'open_compute_runtime',
                          'p0_2_runtime_gate', 'p5_search_gate', 'single_binary', 'new_test', 'ocd',
                          'p3-contract'})
        self.assertNotIn('p3-cf-diff', targets)
        self.assertEqual(
            list(targets)[:3],
            ['open-compute-service.test.cli', 'single-binary', 'p5-search'],
        )
        self.assertTrue(targets['open-compute-service.test.cli'].exclusive)
        self.assertTrue(targets['single-binary'].exclusive)
        self.assertFalse(targets[gate.SERVICE_LIB_TARGET].exclusive)
        self.assertTrue(targets[gate.SERVICE_PROCESS_TARGET].exclusive)
        self.assertEqual(
            targets[gate.SERVICE_LIB_TARGET]._replace(exclusive=True),
            targets[gate.SERVICE_PROCESS_TARGET],
        )
        self.assertTrue(targets['open-compute-runtime.lib.open_compute_runtime'].exclusive)
        self.assertTrue(targets['p0-2'].exclusive)
        self.assertTrue(gate.CARGO_TARGETS['p0-5'][2])
        self.assertTrue(targets['open-compute-service.test.new_test'].exclusive)
        self.assertTrue(all(target.cwd == '/repo/crates/service'
                            for target in targets.values() if target.package_id == 'service'))
        with patch.object(gate.subprocess, 'check_output', return_value=json.dumps(metadata)):
            with self.assertRaisesRegex(ValueError, 'missing registered Gate targets'):
                gate.resolve_targets([], True)

    def test_workspace_rejects_gate_selection_before_build(self):
        with patch.object(gate.sys, 'argv', ['gate.py', '--workspace', 'p0-2']), \
             patch.object(gate, 'resolve_targets') as resolve:
            with self.assertRaisesRegex(ValueError, 'cannot be combined'):
                gate.main()
            resolve.assert_not_called()

    def test_s3_provider_qualification_does_not_require_bun(self):
        metadata = {'workspace_members': [], 'packages': []}
        with patch.object(gate.subprocess, 'check_output', return_value=json.dumps(metadata)), \
             patch.object(gate.shutil, 'which', side_effect=lambda name: '/bin/sh' if name == 'sh' else None):
            targets = gate.resolve_targets(['s3-provider-qualification'], False)
        target = targets['s3-provider-qualification']
        self.assertEqual(target.executable, '/bin/sh')
        self.assertEqual(target.cases, ('production-preflight',))
        self.assertIn('OPEN_COMPUTE_TEST_R2_S3_SECRET_ACCESS_KEY', target.env_allowlist)
        self.assertIn('MBX_CACHE_EXPORT_GROUP', target.env_allowlist)
        self.assertIn('CARGO_INCREMENTAL', target.env_allowlist)

    def test_ai_provider_qualification_forwards_pinned_build_inputs(self):
        metadata = {'workspace_members': [], 'packages': []}
        with patch.object(gate.subprocess, 'check_output', return_value=json.dumps(metadata)), \
             patch.object(gate.shutil, 'which', side_effect=lambda name: '/bin/bun' if name == 'bun' else None):
            targets = gate.resolve_targets(['p5-ai-provider-qualification'], False)
        target = targets['p5-ai-provider-qualification']
        # The harness rebuilds the Rust gate; build.rs requires both pinned runtime inputs.
        self.assertIn('OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE', target.env_allowlist)
        self.assertIn('OPEN_COMPUTE_BUILD_CADDY', target.env_allowlist)
        self.assertIn('MBX_CACHE_EXPORT_GROUP', target.env_allowlist)
        self.assertIn('CARGO_INCREMENTAL', target.env_allowlist)

    def test_final_workspace_runs_complete_inventory_once_and_only_timing_twice_more(self):
        targets = self.targets(['p0-1', 'p1-security', 'p2-1', 'workflow-product'])
        targets['unit'] = gate.Target('package', 'lib', 'lib', '/repo', False)
        targets[gate.SERVICE_LIB_TARGET] = gate.Target(
            'service', 'open_compute_service', 'lib', '/repo', False)
        targets[gate.SERVICE_PROCESS_TARGET] = gate.Target(
            'service', 'open_compute_service', 'lib', '/repo', True)
        plans = gate.round_plan(targets, 3)
        self.assertEqual(plans[0], targets)
        for plan in plans[1:]:
            self.assertEqual(set(plan), {'p0-1', 'workflow-product'})
            self.assertNotIn(gate.SERVICE_LIB_TARGET, plan)
            self.assertNotIn(gate.SERVICE_PROCESS_TARGET, plan)
            for name, target in plan.items():
                self.assertEqual(set(target.cases), set(gate.TIMING[name]))
                self.assertFalse(set(target.cases) & set(gate.ONCE.get(name, ())))
        self.assertEqual(gate.round_plan(targets, 1), [targets])
        deterministic = self.targets(['p1-security', 'p2-1'])
        self.assertEqual(gate.round_plan(deterministic, 3), [deterministic])

    def test_final_main_records_mixed_rounds_and_stops_after_second_round_failure(self):
        for failure in [False, True]:
            selected = ['p1-security', 'workflow-product']
            targets = {name: target._replace(cases=tuple(sorted(
                gate.ONCE.get(name, ()) + gate.TIMING.get(name, ()))))
                for name, target in self.targets(selected).items()}
            calls = []
            def run(planned, artifacts, directory, jobs, *args):
                calls.append(planned)
                return [{'target': name, 'exit_code': int(failure and len(calls) == 3),
                         'cases_passed': len(target.cases)} for name, target in planned.items()]
            with tempfile.TemporaryDirectory() as temp, \
                 patch.object(gate, 'ROOT', Path(temp)), \
                 patch.object(gate, 'verify_inputs', return_value={}), \
                 patch.object(gate, 'validate_contract_case_mapping'), \
                 patch.object(gate, 'write_contract_report'), \
                 patch.object(gate, 'source_identity', return_value='unchanged'), \
                 patch.object(gate, 'resolve_targets', return_value=targets), \
                 patch.object(gate, 'build_targets', return_value=({}, {})), \
                 patch.object(gate, 'verify_case_inventory', return_value=targets), \
                 patch.object(gate.platform, 'platform', return_value='test-host'), \
                 patch.object(gate.subprocess, 'check_output', return_value='revision'), \
                 patch.object(gate.sys, 'argv', ['gate.py', 'p1-security', 'workflow-product']), \
                 patch.dict(os.environ, OPEN_COMPUTE_GATE_ROUNDS='3'), \
                 patch.object(gate, 'run_round', side_effect=run):
                self.assertEqual(gate.main(), int(failure))
                report = json.loads(next(Path(temp).rglob('report.json')).read_text())
            self.assertEqual(set(calls[1]), set(selected))
            for planned in calls[2:]:
                self.assertEqual(set(planned), {'workflow-product'})
                self.assertEqual(set(planned['workflow-product'].cases), set(gate.TIMING['workflow-product']))
            self.assertEqual(len(calls), 3 if failure else 4)
            self.assertTrue(report['inventory_verified'])
            self.assertEqual(report['test_processes_executed'], 3 if failure else 4)

    def test_inventory_failure_prevents_all_product_execution(self):
        with tempfile.TemporaryDirectory() as temp, \
             patch.object(gate, 'ROOT', Path(temp)), \
             patch.object(gate, 'verify_inputs', return_value={}), \
             patch.object(gate, 'validate_contract_case_mapping'), \
             patch.object(gate, 'write_contract_report'), \
             patch.object(gate, 'source_identity', return_value='unchanged'), \
             patch.object(gate, 'resolve_targets', return_value=self.targets(['p0-2'])), \
             patch.object(gate, 'build_targets', return_value=({}, {})), \
             patch.object(gate, 'verify_case_inventory', side_effect=ValueError('registry mismatch')), \
             patch.object(gate.platform, 'platform', return_value='test-host'), \
             patch.object(gate.subprocess, 'check_output', return_value='revision'), \
             patch.object(gate.sys, 'argv', ['gate.py', 'p0-2']), \
             patch.dict(os.environ, OPEN_COMPUTE_GATE_ROUNDS='3'), \
             patch.object(gate, 'run_round', return_value=[{'target': 'p0-2', 'exit_code': 0}]) as run:
            self.assertEqual(gate.main(), 1)
            self.assertEqual(run.call_count, 1)
            report = json.loads(next(Path(temp).rglob('report.json')).read_text())
            self.assertFalse(report['inventory_verified'])
            self.assertEqual(report['test_processes_executed'], 0)

    def test_registry_and_discovery_fail_on_missing_extra_and_ambiguous_cases(self):
        gate.validate_registry(gate.TARGETS)
        with self.assertRaisesRegex(ValueError, 'registry differ'):
            gate.validate_registry({**gate.TARGETS, 'unreviewed': ()})
        with patch.dict(gate.ONCE, {'p0-2': gate.TIMING['p0-2']}):
            with self.assertRaisesRegex(ValueError, 'duplicate'):
                gate.validate_registry(gate.TARGETS)
        for cases in [(), ('same', 'same')]:
            with patch.dict(gate.validate_registry.__globals__,
                            {'SERVICE_PROCESS_CASES': cases}):
                with self.assertRaisesRegex(ValueError, 'workspace case partition'):
                    gate.validate_registry(gate.TARGETS)
        with tempfile.TemporaryDirectory() as temp:
            log = Path(temp)/'list.log'
            targets = self.targets(['p1-security'])
            expected = gate.ONCE['p1-security']
            for cases in [expected, expected[:1], (*expected, 'unreviewed')]:
                log.write_text('\n'.join(f'{name}: test' for name in cases)
                               + f'\n\n{len(cases)} tests, 0 benchmarks\n')
                prepared = [{'target': 'p1-security', 'log': str(log)}]
                if cases == expected:
                    found = gate.verify_case_inventory(targets, prepared)
                    self.assertEqual(found['p1-security'].cases, tuple(sorted(expected)))
                else:
                    with self.assertRaisesRegex(ValueError, 'registry mismatch'):
                        gate.verify_case_inventory(targets, prepared)
            for raw in ['', 'one: test\n\n2 tests, 0 benchmarks\n',
                        'one: test\none: test\n\n2 tests, 0 benchmarks\n',
                        'bench: benchmark\n\n0 tests, 1 benchmark\n']:
                log.write_text(raw)
                with self.assertRaisesRegex(ValueError, 'inventory'):
                    gate.discovered_cases(log)
            log.write_text('0 tests, 0 benchmarks\n')
            self.assertEqual(gate.discovered_cases(log), ())

    def test_service_library_inventory_is_partitioned_once_before_execution(self):
        cases = (*gate.SERVICE_PROCESS_CASES, 'ordinary::case')
        targets = {
            gate.SERVICE_LIB_TARGET:
                gate.Target('service', 'open_compute_service', 'lib', '/repo', False),
            gate.SERVICE_PROCESS_TARGET:
                gate.Target('service', 'open_compute_service', 'lib', '/repo', True),
        }
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            full = root / 'full.log'
            full.write_text('\n'.join(f'{name}: test' for name in cases)
                            + f'\n\n{len(cases)} tests, 0 benchmarks\n')
            prepared = [
                {'target': name, 'log': str(full)} for name in targets
            ]
            partitioned = gate.verify_case_inventory(targets, prepared)
            ordinary = set(partitioned[gate.SERVICE_LIB_TARGET].cases)
            process = set(partitioned[gate.SERVICE_PROCESS_TARGET].cases)
            self.assertEqual(ordinary, {'ordinary::case'})
            self.assertEqual(process, set(gate.SERVICE_PROCESS_CASES))
            self.assertFalse(ordinary & process)
            self.assertEqual(ordinary | process, set(cases))

            with self.assertRaisesRegex(ValueError, 'planned together'):
                gate.verify_case_inventory(
                    {gate.SERVICE_LIB_TARGET: targets[gate.SERVICE_LIB_TARGET]},
                    prepared[:1],
                )

            different = root / 'different.log'
            different.write_text('\n'.join(f'{name}: test' for name in cases[:-1])
                                 + f'\n\n{len(cases) - 1} tests, 0 benchmarks\n')
            with self.assertRaisesRegex(ValueError, 'different inventories'):
                gate.verify_case_inventory(targets, [
                    prepared[0],
                    {'target': gate.SERVICE_PROCESS_TARGET, 'log': str(different)},
                ])

            missing = root / 'missing.log'
            reduced = cases[1:]
            missing.write_text('\n'.join(f'{name}: test' for name in reduced)
                               + f'\n\n{len(reduced)} tests, 0 benchmarks\n')
            with self.assertRaisesRegex(ValueError, 'exclusive cases are missing'):
                gate.verify_case_inventory(targets, [
                    {'target': name, 'log': str(missing)} for name in targets
                ])

    def test_contract_mapping_validates_member_runtime_evidence_before_build(self):
        registered = f'p0-2::{gate.TIMING["p0-2"][0]}'
        with tempfile.TemporaryDirectory() as temp, patch.object(gate, 'ROOT', Path(temp)):
            root = Path(temp) / 'test/conformance'
            root.mkdir(parents=True)
            catalog = {
                'schemaVersion': 1,
                'contracts': [{
                    'positiveCases': [registered],
                    'negativeCases': [registered],
                }],
                'memberEvidence': [{ 'runtimeCases': [registered] }],
            }
            (root / 'catalog.json').write_text(json.dumps(catalog))
            gate.validate_contract_case_mapping()
            catalog['memberEvidence'][0]['runtimeCases'] = ['p0-2::missing']
            (root / 'catalog.json').write_text(json.dumps(catalog))
            with self.assertRaisesRegex(ValueError, 'unregistered Gate cases'):
                gate.validate_contract_case_mapping()
            catalog['memberEvidence'][0]['runtimeCases'] = [registered, registered]
            (root / 'catalog.json').write_text(json.dumps(catalog))
            with self.assertRaisesRegex(ValueError, 'member runtime evidence'):
                gate.validate_contract_case_mapping()

    def test_exact_case_selection_rejects_zero_passes_ignored_and_partial_execution(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(gate, 'ROOT', Path(temp)):
            target = self.targets(['p0-2'])['p0-2']._replace(cases=('case', 'case::nested'))
            for index, (passed, ignored, expected) in enumerate([(2, 0, 0), (0, 0, 1),
                                                               (1, 0, 1), (1, 1, 1)]):
                def execute(command, **kwargs):
                    self.assertEqual(command[-3:], ['--exact', 'case', 'case::nested'])
                    kwargs['stdout'].write(f'test result: ok. {passed} passed; 0 failed; '
                                           f'{ignored} ignored; 0 measured; 3 filtered out;\n')
                    return SimpleNamespace(returncode=0)
                with patch.object(gate.subprocess, 'run', side_effect=execute):
                    result = gate.execute_target('p0-2', '/compiled/test', Path(temp)/str(index), target)
                self.assertEqual(result['exit_code'], expected)

    def test_final_rounds_reject_coverage_before_build(self):
        for variable, value in [('RUSTFLAGS', '-C instrument-coverage'),
                                ('CARGO_ENCODED_RUSTFLAGS', '-Cinstrument-coverage'),
                                ('CARGO_LLVM_COV', '1')]:
            with patch.object(gate.sys, 'argv', ['gate.py', '--workspace']), \
                 patch.dict(os.environ, {'OPEN_COMPUTE_GATE_ROUNDS': '3', variable: value}), \
                 patch.object(gate, 'resolve_targets') as resolve:
                with self.assertRaisesRegex(ValueError, 'uninstrumented'):
                    gate.main()
                resolve.assert_not_called()

    def test_process_gate_requires_lsof_before_runtime_checks_or_build(self):
        targets = self.targets(['p0-1'])
        with patch.object(gate.shutil, 'which', return_value=None), \
             patch.object(gate, 'verify_inputs') as runtime:
            with self.assertRaisesRegex(ValueError, 'p0-1 requires lsof'):
                gate.verify_selected_inputs(targets)
            runtime.assert_not_called()
        for executable in ('/usr/sbin/lsof', 'lsof'):
            with patch.object(gate.shutil, 'which', side_effect=lambda name: name if name == executable else None), \
                 patch.object(gate, 'verify_inputs', return_value={'verified': True}):
                self.assertEqual(gate.verify_selected_inputs(targets), {'verified': True})
        with patch.object(gate.shutil, 'which') as tool, \
             patch.object(gate, 'verify_inputs', return_value={'verified': True}):
            self.assertEqual(gate.verify_selected_inputs(self.targets(['p0-2'])), {'verified': True})
            tool.assert_not_called()

    def test_coverage_rejects_extra_rounds_before_tool_checks_or_cleanup(self):
        source = (gate.ROOT/'test/coverage.sh').read_text()
        self.assertIn(
            '/src/bin/(s3_fixture|s3_provider_qualification|supervisor_fixture|install_upgrade_fixture)\\.rs$', source)
        result = subprocess.run([str(gate.ROOT/'test/coverage.sh')], capture_output=True,
                                env={'OPEN_COMPUTE_GATE_ROUNDS': '3', 'PATH': '/usr/bin:/bin'},
                                timeout=10)
        self.assertEqual(result.returncode, 1)
        self.assertIn('coverage runs exactly once', result.stderr.decode())

    def test_coverage_rejects_invalid_html_mode_before_tool_checks(self):
        result = subprocess.run([str(gate.ROOT/'test/coverage.sh')], capture_output=True,
                                env={'OPEN_COMPUTE_COVERAGE_HTML': 'false',
                                     'PATH': '/usr/bin:/bin'}, timeout=10)
        self.assertEqual(result.returncode, 1)
        self.assertIn('OPEN_COMPUTE_COVERAGE_HTML must be 0 or 1',
                      result.stderr.decode())

    def test_coverage_run_keeps_old_profiles_and_reports_out_of_current_inputs(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / 'test').mkdir()
            script = root / 'test/coverage.sh'
            shutil.copy2(gate.ROOT / 'test/coverage.sh', script)
            cache = root / 'target/llvm-cov-target'
            cache.mkdir(parents=True)
            old_profile = cache / 'old.profraw'
            old_profile.write_text('old profile')
            reports = root / 'target/llvm-cov'
            reports.mkdir()
            (reports / 'summary.json').write_text('old report')
            workerd = root / 'workerd'
            workerd.write_text('verified runtime input belongs to the real Gate')
            tools = {}
            sources = {
                'mbx': """import sys
if sys.argv[1:3] == ['llvm-cov', 'show-env']:
    print('export CARGO_LLVM_COV=1')
elif sys.argv[1:] not in (['llvm-cov', '--version'], ['fetch', '--locked']):
    sys.exit(9)
""",
                'rustc': """import sys
print('host: fixture' if sys.argv[1:] == ['-vV'] else '/unused')
""",
                'llvm-profdata': """import sys
from pathlib import Path
args=sys.argv[1:]
Path(args[args.index('-o')+1]).write_text('merged current profiles')
""",
                'llvm-cov': """import os,sys,json,re
pattern, = [arg.split('=',1)[1] for arg in sys.argv if arg.startswith('--ignore-filename-regex=')]
for kind in ('registry', 'git'):
    assert re.search(pattern, os.environ['CARGO_HOME']+'/'+kind+'/dependency/src/lib.rs')
assert not re.search(pattern, os.environ['FIXTURE_WORKSPACE']+'/crates/service/src/main.rs')
print('TN:current' if '--format=lcov' in sys.argv else json.dumps({'data':[{'totals':{'lines':{'percent':float(os.environ['FIXTURE_COVERAGE'])}}}]}))
""",
                'gate': """import os,sys
from pathlib import Path
if '--list' not in sys.argv:
    run=Path(os.environ['OPEN_COMPUTE_COVERAGE_RUN_DIR'])
    (run/'objects').mkdir()
    (run/'objects/current-binary').write_text('exact current object')
    (run/'profiles/current.profraw').write_text('current profile')
""",
            }
            for name, source in sources.items():
                path = root / name
                path.write_text(f'#!{sys.executable}\n' + source)
                path.chmod(0o700)
                tools[name] = str(path)
            shutil.copy2(tools['gate'], root / 'test/gate.py')
            environment = {'PATH': str(root) + os.pathsep + os.environ['PATH'],
                           'RUSTC': tools['rustc'], 'LLVM_COV': tools['llvm-cov'],
                           'LLVM_PROFDATA': tools['llvm-profdata'],
                           'OPEN_COMPUTE_TEST_WORKERD': str(workerd),
                           'OPEN_COMPUTE_COVERAGE_HTML': '0', 'FIXTURE_COVERAGE': '100',
                           'CARGO_HOME': str(root / 'cargo+[home]'), 'FIXTURE_WORKSPACE': str(root)}
            result = subprocess.run([str(script)], capture_output=True, env=environment, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            runs = list((root / '.temp/coverage').iterdir())
            self.assertEqual(len(runs), 1)
            run = runs[0]
            self.assertEqual((run / 'profraw-list').read_text().splitlines(),
                             [str(run / 'profiles/current.profraw')])
            self.assertEqual((run / 'previous-reports/summary.json').read_text(), 'old report')
            successful_report = (reports / 'summary.json').read_bytes()
            self.assertEqual((run / 'reports/summary.json').read_bytes(), successful_report)
            self.assertEqual(old_profile.read_text(), 'old profile')
            environment['FIXTURE_COVERAGE'] = '89'
            result = subprocess.run([str(script)], capture_output=True, env=environment, timeout=10)
            self.assertEqual(result.returncode, 1)
            self.assertIn('below 90.00%', result.stderr.decode())
            self.assertEqual((reports / 'summary.json').read_bytes(), successful_report)
            self.assertEqual(old_profile.read_text(), 'old profile')
            self.assertEqual(len(list((root / '.temp/coverage').iterdir())), 2)

    def test_source_freeze_ignores_designs_and_python_caches_but_includes_runtime_inputs(self):
        names = ['crates/service/src/resources.rs', 'docs/references/runbooks/install.md',
                 'docs/plan.md', 'docs/implemented/report.md',
                 'test/__pycache__/gate.cpython-314.pyc']
        with tempfile.TemporaryDirectory() as temp, \
             patch.object(gate, 'ROOT', Path(temp)), \
             patch.object(gate.subprocess, 'check_output',
                          return_value='\0'.join(names).encode()) as discovery:
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
            self.assertEqual(discovery.call_args.args[0][1:3], ['-c', 'core.excludesFile=/dev/null'])

    def test_build_matches_package_and_kind_and_rejects_missing_executables(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            binary = root / 'test-binary'
            binary.write_bytes(b'compiled test')
            targets = {
                gate.SERVICE_LIB_TARGET: gate.Target('one', 'same', 'lib', temp, False),
                gate.SERVICE_PROCESS_TARGET: gate.Target('one', 'same', 'lib', temp, True),
                'second': gate.Target('two', 'same', 'test', temp, False),
            }
            messages = [
                {'reason': 'compiler-artifact', 'package_id': target.package_id,
                 'target': {'name': target.name, 'kind': [target.kind], 'test': True},
                 'profile': {'test': True}, 'executable': str(binary)}
                for target in (targets[gate.SERVICE_LIB_TARGET], targets['second'])
            ]
            messages.append({'reason': 'compiler-artifact', 'package_id': 'fixture-package',
                             'target': {'name': 'fixture', 'kind': ['bin'], 'test': False},
                             'profile': {'test': True}, 'executable': str(binary)})
            result = SimpleNamespace(returncode=0, stdout='\n'.join(map(json.dumps, messages)))
            with patch.object(gate.subprocess, 'run', return_value=result) as run:
                artifacts, build = gate.build_targets(targets, root, True)
            self.assertEqual(set(artifacts), set(targets))
            self.assertEqual(artifacts[gate.SERVICE_LIB_TARGET],
                             artifacts[gate.SERVICE_PROCESS_TARGET])
            self.assertEqual(build['executables'][gate.SERVICE_LIB_TARGET],
                             build['executables'][gate.SERVICE_PROCESS_TARGET])
            self.assertEqual(build['invocations'], 1)
            self.assertIn('--all-targets', run.call_args.args[0])
            missing = root / 'missing'
            missing.mkdir()
            result.stdout = json.dumps(messages[0])
            with patch.object(gate.subprocess, 'run', return_value=result):
                with self.assertRaisesRegex(RuntimeError, 'did not produce'):
                    gate.build_targets(targets, missing, True)
            unplanned = root / 'unplanned'
            unplanned.mkdir()
            result.stdout = json.dumps({
                'reason': 'compiler-artifact', 'package_id': 'fixture-package',
                'target': {'name': 'fixture', 'kind': ['bin'], 'test': True},
                'profile': {'test': True}, 'executable': str(binary),
            })
            with patch.object(gate.subprocess, 'run', return_value=result):
                with self.assertRaisesRegex(RuntimeError, 'unplanned test executable'):
                    gate.build_targets(targets, unplanned, True)
            duplicate = root / 'duplicate'
            duplicate.mkdir()
            with patch.object(gate.subprocess, 'run') as run, \
                 self.assertRaisesRegex(RuntimeError, 'duplicate Gate owners'):
                gate.build_targets(
                    {
                        'one': gate.Target('one', 'same', 'lib', temp, False),
                        'two': gate.Target('one', 'same', 'lib', temp, False),
                    },
                    duplicate,
                    True,
                )
            run.assert_not_called()

    def test_coverage_build_keeps_prior_objects_and_refuses_overwrite(self):
        self.assert_coverage_build_preservation()

    def test_coverage_build_preserves_linux_clones_when_the_cache_changes(self):
        with patch.object(gate.sys, 'platform', 'linux'):
            self.assert_coverage_build_preservation()

    def test_coverage_build_preserves_objects_when_apfs_cloning_is_unavailable(self):
        with patch.object(gate.sys, 'platform', 'darwin'), \
             patch.object(gate.subprocess, 'check_call',
                          side_effect=subprocess.CalledProcessError(1, ['/bin/cp', '-c'])):
            self.assert_coverage_build_preservation()

    def assert_coverage_build_preservation(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            binary = root / 'test-binary'
            binary.write_bytes(b'compiled instrumented test')
            prior = root / 'prior-objects'
            prior.mkdir()
            retained = prior / 'earlier-binary'
            retained.write_bytes(b'retained failure evidence')
            package = f'path+file://{gate.ROOT}/crates/service#open-compute-service@0.2.3'
            targets = {'one': gate.Target(package, 'test', 'test', temp, False)}
            messages = [{'reason': 'compiler-artifact', 'package_id': package,
                         'target': {'name': 'test', 'kind': ['test'], 'test': True},
                         'profile': {'test': True}, 'executable': str(binary)}]
            result = SimpleNamespace(returncode=0, stdout='\n'.join(map(json.dumps, messages)))
            fresh = root / 'fresh'
            fresh.mkdir()
            environment = {'CARGO_LLVM_COV': '1', 'OPEN_COMPUTE_COVERAGE_RUN_DIR': str(fresh)}
            with patch.object(gate.subprocess, 'run', return_value=result), \
                 patch.dict(os.environ, environment):
                gate.build_targets(targets, fresh, True)
                objects = list((fresh / 'objects').iterdir())
                self.assertEqual(len(objects), 1)
                self.assertEqual(objects[0].read_bytes(), binary.read_bytes())
                self.assertNotEqual(objects[0].stat().st_ino, binary.stat().st_ino)
                binary.write_bytes(b'rebuilt cache binary')
                self.assertEqual(objects[0].read_bytes(), b'compiled instrumented test')
                self.assertEqual(retained.read_bytes(), b'retained failure evidence')
                second = root / 'second'
                second.mkdir()
                with self.assertRaisesRegex(RuntimeError, 'refusing to overwrite coverage objects'):
                    gate.build_targets(targets, second, True)
                self.assertEqual(objects[0].read_bytes(), b'compiled instrumented test')
                link = root / 'linked-run'
                link.symlink_to(fresh, target_is_directory=True)
                third = root / 'third'
                third.mkdir()
                with patch.dict(os.environ, {'OPEN_COMPUTE_COVERAGE_RUN_DIR': str(link)}), \
                     self.assertRaisesRegex(RuntimeError, 'not an owned directory'):
                    gate.build_targets(targets, third, True)
                fourth = root / 'fourth'
                fourth.mkdir()
                with patch.dict(os.environ, {'OPEN_COMPUTE_COVERAGE_RUN_DIR': 'relative'}), \
                     self.assertRaisesRegex(RuntimeError, 'not an owned directory'):
                    gate.build_targets(targets, fourth, True)

    def test_typed_discovery_and_exact_execution_use_strict_json_and_environment(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(gate, 'ROOT', Path(temp)):
            log = Path(temp) / 'typed.log'
            log.write_text('{"schemaVersion":1,"cases":["second","first"]}\n')
            self.assertEqual(gate.discovered_cases(log, 'bun-test'), ('first', 'second'))
            for value in ['', '[]', '{"schemaVersion":1,"cases":[]}',
                          '{"schemaVersion":1,"cases":["same","same"]}', 'not-json']:
                log.write_text(value)
                with self.assertRaisesRegex(ValueError, 'typed target inventory'):
                    gate.discovered_cases(log, 'bun-test')

            target = gate.TypedTarget(
                None, 'p3-contract', 'bun-test', temp, False, ('first', 'second'),
                '/usr/bin/bun', ('/repo/check.ts',), ('--list',), ('PATH', 'ALLOWED'),
                30, 'light', 'contract-checker')

            def execute(command, **kwargs):
                self.assertEqual(command, ['/usr/bin/bun', '/repo/check.ts',
                                           '--case', 'first', '--case', 'second'])
                self.assertEqual(kwargs['env']['ALLOWED'], 'yes')
                self.assertNotIn('SECRET', kwargs['env'])
                self.assertEqual(kwargs['env']['BUN_RUNTIME_TRANSPILER_CACHE_PATH'], '0')
                self.assertEqual(kwargs['env']['NODE_DISABLE_COMPILE_CACHE'], '1')
                kwargs['stdout'].write(json.dumps({
                    'schemaVersion': 1,
                    'status': 'passed',
                    'cases': [
                        {'id': 'first', 'status': 'passed'},
                        {'id': 'second', 'status': 'passed'},
                    ],
                }) + '\n')
                kwargs['stderr'].write('mbx: diagnostic after the JSON result\n')
                return SimpleNamespace(returncode=0)

            with patch.dict(os.environ, {'PATH': '/usr/bin', 'ALLOWED': 'yes',
                                          'SECRET': 'must-not-pass'}, clear=True), \
                 patch.object(gate.subprocess, 'run', side_effect=execute):
                result = gate.execute_target(
                    'p3-contract', '/usr/bin/bun', Path(temp) / 'execute', target)
            self.assertEqual(result['exit_code'], 0)
            self.assertEqual(result['cases_passed'], 2)
            self.assertEqual((Path(temp) / 'execute/stderr.log').read_text(),
                             'mbx: diagnostic after the JSON result\n')

    def test_contract_report_keeps_local_and_remote_verdicts_separate(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(gate, 'ROOT', Path(temp)):
            root = Path(temp)
            (root / 'test/conformance').mkdir(parents=True)
            (root / 'test/conformance/baseline.json').write_text('{}')
            (root / 'test/conformance/catalog.json').write_text(json.dumps({
                'schemaVersion': 1,
                'contracts': [{
                    'id': 'contract', 'product': 'workers', 'status': 'supported',
                    'positiveCases': ['local::positive'],
                    'negativeCases': ['local::negative'], 'deviations': [],
                }],
            }))
            report = {
                'source_sha256': 'source',
                'results': [{'targets': [{
                    'target': 'local', 'exit_code': 0,
                    'cases': ['positive', 'negative'],
                }]}],
            }
            gate.write_contract_report(root, report)
            contract = json.loads((root / 'contract-report.json').read_text())
            self.assertEqual(contract['localVerdict'], 'contract_go')
            self.assertEqual(contract['cloudflareDifferential'], 'not_qualified')
            self.assertEqual(contract['platformVerdict'], 'conditional_go')

            diff = root / 'diff-report.json'
            diff.write_text('{"schemaVersion":1,"status":"passed"}')
            report['results'][0]['targets'].append({
                'target': 'p3-cf-diff', 'exit_code': 0, 'cases': ['portable'],
                'diff_report': str(diff),
            })
            gate.write_contract_report(root, report)
            contract = json.loads((root / 'contract-report.json').read_text())
            self.assertEqual(contract['cloudflareDifferential'], 'qualified')
            self.assertEqual(contract['platformVerdict'], 'go')


if __name__ == '__main__':
    unittest.main()
