"""Keep live Gate status available without exposing captured process output."""
from contextlib import redirect_stdout
from concurrent.futures import ThreadPoolExecutor
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

import test_with_ci_broker

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import gate
sys.path.pop(0)

broker = test_with_ci_broker.fixture.broker


class Progress(unittest.TestCase):
    def setUp(self):
        self.parent = Path(__file__).resolve().parents[2] / '.temp/gate-progress-tests'
        self.parent.mkdir(parents=True, exist_ok=True)
        self.environment = patch.dict(os.environ, GITHUB_ACTIONS='true')
        self.environment.start()
        self.addCleanup(self.environment.stop)

    def record(self, status, seconds=None):
        output = io.StringIO()
        with patch.dict(os.environ, GITHUB_ACTIONS='true'), redirect_stdout(output):
            gate.ci_progress('p23-containers', 'test', status, seconds)
        return output.getvalue().encode()

    def test_parallel_target_status_records_are_complete_lines(self):
        class YieldingOutput(io.StringIO):
            def write(self, text):
                result = super().write(text)
                if text != '\n':
                    time.sleep(0.001)
                return result

        output = YieldingOutput()
        with redirect_stdout(output), ThreadPoolExecutor(max_workers=4) as executor:
            futures = [executor.submit(gate.ci_progress, 'p23-containers', 'test', 'START')
                       for _ in range(16)]
            for future in futures:
                future.result()
        rows = output.getvalue().splitlines()
        self.assertEqual(len(rows), 16)
        self.assertTrue(all(broker.gate_progress(line.encode()) is not None for line in rows))

    def test_only_complete_bounded_status_records_are_forwarded(self):
        line = self.record('PASS', 1.25)
        self.assertEqual(json.loads(broker.gate_progress(line).split(' ', 1)[1]),
                         dict(target='p23-containers', phase='test', status='PASS', seconds=1.25))
        value = json.loads(line.split(b' ', 1)[1])
        for bad in [dict(value, message='private-token'), dict(value, target='/private/key'),
                    dict(value, phase=['test']), dict(value, status='UNKNOWN'),
                    dict(value, seconds=True), dict(value, seconds=-1),
                    dict(value, seconds=float('nan')), dict(value, seconds=10**1000),
                    dict(value, target='a' * 5000)]:
            self.assertIsNone(broker.gate_progress(b'CI_GATE_PROGRESS ' + json.dumps(bad).encode()))
        self.assertIsNone(broker.gate_progress(b'private upstream exception and token'))
        self.assertIsNone(broker.gate_progress(
            line.rstrip()[:-1] + b',"target":"other"}'))

    def test_running_command_status_is_live_and_original_output_is_preserved(self):
        started, finished = self.record('START'), self.record('FAIL', 2.5)
        output = io.StringIO()
        before = set(threading.enumerate())
        with tempfile.TemporaryDirectory(dir=self.parent) as directory:
            path = Path(directory) / 'command.log'
            with path.open('xb') as log, redirect_stdout(output), broker.relay_gate_progress(log):
                log.write(b'private-token\n' + started)
                log.flush()
                deadline = time.monotonic() + 2
                while 'START' not in output.getvalue() and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertIn('START', output.getvalue())
                log.write(b'x' * 9000 + b'\n' + finished[:20])
                log.flush()
                time.sleep(0.15)
                log.write(finished[20:])
            expected = b'private-token\n' + started + b'x' * 9000 + b'\n' + finished
            self.assertEqual(path.read_bytes(), expected)
        self.assertNotIn('private-token', output.getvalue())
        rows = [json.loads(line.split(' ', 1)[1]) for line in output.getvalue().splitlines()]
        self.assertEqual([row['status'] for row in rows], ['START', 'FAIL'])
        self.assertEqual(set(threading.enumerate()), before)

    def test_replaced_log_fails_closed_and_joins_the_observer(self):
        before = set(threading.enumerate())
        with tempfile.TemporaryDirectory(dir=self.parent) as directory:
            path = Path(directory) / 'command.log'
            with path.open('xb') as log, self.assertRaisesRegex(
                    ValueError, 'GATE_PROGRESS_LOG_IDENTITY_CHANGED'):
                with broker.relay_gate_progress(log):
                    path.rename(path.with_suffix('.original'))
                    path.write_bytes(b'private replacement')
        self.assertEqual(set(threading.enumerate()), before)


if __name__ == '__main__':
    unittest.main()
