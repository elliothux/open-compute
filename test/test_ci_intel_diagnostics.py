"""Ownership and fail-closed checks for the temporary Intel observer."""
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import unittest

PATH = Path(__file__).with_name('ci_intel_diagnostics.py')
SPEC = importlib.util.spec_from_file_location('intel_diagnostics', PATH)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ObserverTests(unittest.TestCase):
    def test_only_same_owner_descendants_are_visible(self):
        rows = '''10 1 501 00:01 1.0 50 S /test/harness
11 10 501 00:01 2.0 60 S /test/workerd
12 11 501 00:01 1.0 70 S /path with spaces/ocd
13 10 0 00:01 0.0 80 S /root/private
14 1 501 00:01 0.0 90 S /unrelated/application
15 16 501 00:01 0.0 90 S /cycle/a
16 15 501 00:01 0.0 90 S /cycle/b
malformed row
'''
        result = MODULE.owned_processes(rows, 10, 501)
        self.assertEqual({row['pid'] for row in result}, {10, 11, 12})
        self.assertEqual(next(row['image'] for row in result if row['pid'] == 12),
                         '/path with spaces/ocd')
        self.assertEqual(MODULE.owned_processes(rows, 100, 501), [])
        self.assertEqual(MODULE.owned_processes(rows.replace('10 1 501', '10 1 0'), 10, 501), [])

    def test_stopped_observer_does_not_touch_the_child(self):
        with tempfile.TemporaryDirectory() as directory:
            child = subprocess.Popen(['/bin/sleep', '30'])
            try:
                stop = threading.Event()
                stop.set()
                MODULE.observe(child, Path(directory), 'r2', stop)
                self.assertIsNone(child.poll())
                self.assertEqual((Path(directory) / 'processes.jsonl').read_text(), '')
            finally:
                child.terminate()
                child.wait()

    def test_source_mismatch_fails_before_any_build(self):
        env = dict(os.environ, DIAGNOSTIC_SOURCE_REVISION='0' * 40)
        result = subprocess.run(['python3', str(PATH), '--mode', 'r2'],
                                env=env, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('AssertionError', result.stderr)
        self.assertNotIn('Compiling', result.stdout + result.stderr)

    def test_only_the_three_failed_cases_are_selected(self):
        self.assertEqual(len(MODULE.CASES['r2'][1]), 1)
        self.assertEqual(len(MODULE.CASES['lifecycle'][1]), 2)
        self.assertNotIn('readonly_commands_need_only_the_single_executable',
                         MODULE.CASES['lifecycle'][1])


if __name__ == '__main__':
    unittest.main()
