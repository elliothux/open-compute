"""Input retention must detect replacement even while the original file stays open."""

import importlib.util
import io
import os
from pathlib import Path
import sys
import tempfile
import tarfile
import unittest
from collections import namedtuple
from unittest import mock

spec = importlib.util.spec_from_file_location('diagnostic_inputs',
                                             Path(__file__).with_name('diagnostic-inputs.py'))
diagnostic = importlib.util.module_from_spec(spec)
spec.loader.exec_module(diagnostic)
native_spec = importlib.util.spec_from_file_location('native_http_diagnostic',
    Path(__file__).with_name('native-http-diagnostic.py'))
native = importlib.util.module_from_spec(native_spec)
native_spec.loader.exec_module(native)


class DiagnosticInputsTest(unittest.TestCase):
    def test_final_guard_retains_evidence_and_stops_its_process_at_disk_waterline(self):
        disk = namedtuple('Disk', 'total used free')
        total = 100 << 30
        for mode, full in [(mode, full) for mode in ('workspace-final', 'outbound-contexts')
                           for full in (False, True)]:
            with self.subTest(mode=mode, full=full), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                binary = root / 'binary'
                binary.write_bytes(b'input')
                initial = disk(total, 10 << 30, 90 << 30)
                observed = disk(total, 96 << 30, 4 << 30) if full else initial
                measurements = iter([initial])

                def measure(_):
                    return next(measurements, observed)

                command = [sys.executable, '-c', 'import time; time.sleep(0.5)']
                environment = dict.fromkeys(('OPEN_COMPUTE_TEST_WORKERD',
                    'OPEN_COMPUTE_BUILD_CADDY', 'OPEN_COMPUTE_TEST_CONTAINERS_INPUT'), str(binary))
                with (mock.patch.object(Path, 'cwd', return_value=root),
                      mock.patch.object(sys, 'argv', ['diagnostic-inputs.py', '--binary',
                          str(binary), '--' + mode]),
                      mock.patch.dict(os.environ, environment, clear=True),
                      mock.patch.object(diagnostic.subprocess, 'check_output', return_value=b''),
                      mock.patch.object(diagnostic.shutil, 'disk_usage', side_effect=measure),
                      mock.patch.object(diagnostic.resource, 'setrlimit'),
                      mock.patch.object(diagnostic, 'directory_usage', return_value={}),
                      mock.patch.object(diagnostic, 'gate_commands', return_value=[command]) as commands):
                    with self.assertRaises(SystemExit) as result:
                        diagnostic.main()
                    self.assertNotIn('LLVM_PROFILE_FILE', os.environ)
                self.assertEqual(result.exception.code, 1 if full else 0)
                commands.assert_called_once_with(mode == 'workspace-final', mode == 'outbound-contexts')
                report = diagnostic.json.loads((root /
                    f'.temp/{mode}-diagnostic/input-guard.json').read_text())
                self.assertEqual(report['success'], not full)
                self.assertTrue(report['heldAndCurrentPathVerified'])
                self.assertTrue(report['allDescriptorsClosed'])
                self.assertGreater(report['observations'], 0)
                self.assertEqual(report['observerFailures'],
                    ['DIAGNOSTIC_DISK_STOP_THRESHOLD'] if full else [])

    def test_admission_keeps_build_budget_above_strict_five_percent_reserve(self):
        disk = namedtuple('Disk', 'total used free')
        total = 100 << 30
        boundary = (44 << 30) + total // 20
        with self.assertRaisesRegex(RuntimeError, 'DISK_ADMISSION_FAILED'):
            diagnostic.disk_admission(disk(total, total - boundary, boundary))
        self.assertEqual(diagnostic.disk_admission(
            disk(total, total - boundary - 1, boundary + 1)), boundary + 1)

    def test_final_preserves_workspace_final_and_controlled_egress(self):
        discovery, acceptance = diagnostic.gate_commands(True)
        self.assertEqual(discovery, acceptance + ['--list'])
        self.assertEqual(acceptance, ['./test/test-p0-2-egress-linux.sh',
            '--workspace', '--final', '--jobs', '2'])
        self.assertNotIn('--keep-going', acceptance)
        self.assertEqual(diagnostic.gate_commands(False), [
            ['./test/gate.py', 'p23-containers', '--list'],
            ['./test/gate.py', 'p23-containers', '--keep-going', '--jobs', '1']])

    def test_outbound_contexts_collect_all_affected_native_targets_in_one_batch(self):
        discovery, execution = diagnostic.gate_commands(False, True)
        self.assertEqual(discovery, execution + ['--list'])
        self.assertEqual(execution, ['./test/test-p0-2-egress-linux.sh',
            'p0-2', 'p0-8', 'workflow-runtime', '--keep-going', '--jobs', '1'])

    def test_storage_measurement_retains_only_owned_directory_sizes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / 'target/debug/incremental'
            path.mkdir(parents=True)
            (path / 'object').write_bytes(b'x' * 8192)
            sizes = diagnostic.directory_usage(root)
            self.assertEqual(set(sizes), {'target/debug/incremental'})
            self.assertGreaterEqual(sizes['target/debug/incremental'], 8192)

    def test_native_archive_preserves_internal_license_links_and_rejects_escape(self):
        for target, accepted in [('../LICENSE', True), ('../../../outside', False)]:
            with self.subTest(target=target), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                archive = root / 'dependency.tar'
                with tarfile.open(archive, 'w') as bundle:
                    license = tarfile.TarInfo('capnproto-capnproto-abcd/LICENSE')
                    license.size = 7
                    bundle.addfile(license, io.BytesIO(b'license'))
                    link = tarfile.TarInfo('capnproto-capnproto-abcd/c++/LICENSE.txt')
                    link.type = tarfile.SYMTYPE
                    link.linkname = target
                    bundle.addfile(link)
                destination = root / 'extracted'
                destination.mkdir()
                spec = {'strip_prefix': 'capnproto-capnproto-abcd/c++'}
                if accepted:
                    source = native.extract_dependency(archive, spec, destination)
                    self.assertEqual((source / 'LICENSE.txt').read_bytes(), b'license')
                    self.assertEqual((source / 'LICENSE.txt').resolve(),
                                     (destination / 'LICENSE').resolve())
                else:
                    with self.assertRaises(tarfile.LinkOutsideDestinationError):
                        native.extract_dependency(archive, spec, destination)

    def test_native_dependency_rejects_unpinned_or_foreign_archives(self):
        source = (Path(__file__).resolve().parents[2] /
            'third_party/workerd/build/deps/gen/deps.MODULE.bazel').read_text()
        pinned = native.dependency(source)
        self.assertRegex(pinned['sha256'], r'^[0-9a-f]{64}$')
        for invalid in (source.replace(pinned['url'], 'https://example.invalid/archive'),
                        source.replace(pinned['sha256'], 'missing'),
                        source.replace(pinned['strip_prefix'], '../outside')):
            with self.subTest(source=invalid[:32]):
                with self.assertRaisesRegex(ValueError, 'CAPNP_DEPENDENCY_INVALID'):
                    native.dependency(invalid)

    def test_replacement_and_byte_drift_are_rejected_and_descriptors_close(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'input'
            path.write_bytes(b'original')
            held = diagnostic.HeldInputs([path])
            fd = held.entries[0][1]
            held.verify(content=True)
            replacement = path.with_name('replacement')
            replacement.write_bytes(b'original')
            replacement.replace(path)
            with self.assertRaisesRegex(RuntimeError, 'IDENTITY_CHANGED'):
                held.verify(content=True)
            held.close()
            with self.assertRaises(OSError):
                os.fstat(fd)
            held = diagnostic.HeldInputs([path])
            try:
                path.write_bytes(b'modified')
                with self.assertRaisesRegex(RuntimeError, 'IDENTITY_CHANGED'):
                    held.verify(content=True)
            finally:
                held.close()


if __name__ == '__main__':
    unittest.main()
