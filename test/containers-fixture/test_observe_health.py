"""Check bounded health observations without exposing Engine output or identity."""
import hashlib
import http.server
import importlib.util
import io
import json
from pathlib import Path
import socketserver
import tempfile
import threading
import unittest
from contextlib import redirect_stdout
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('observe_health', HERE / 'observe-health.py')
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class HealthRecords(unittest.TestCase):
    def test_health_output_and_engine_identity_never_enter_the_record(self):
        secret = 'private-health-output-and-token'
        value = {'Id': secret, 'Name': secret, 'Config': {'Env': [secret]},
                 'State': {'Running': True, 'Health': {'Status': 'starting', 'FailingStreak': 0,
                     'Log': [{'Start': '2026-10-27T00:00:00.000000000Z',
                              'End': '2026-10-27T00:00:01.500000000Z', 'ExitCode': -1,
                              'Output': secret + ' Health check exceeded timeout'}]}}}
        result = fixture.health_record(value)
        self.assertNotIn(secret, json.dumps(result))
        self.assertEqual(result['checks'][0]['durationMs'], 1500)
        self.assertEqual(result['checks'][0]['exitCode'], -1)
        self.assertEqual(result['checks'][0]['outputSha256'],
                         hashlib.sha256(value['State']['Health']['Log'][0]['Output'].encode()).hexdigest())
        self.assertEqual(result['checks'][0]['knownErrors'], ['Health check exceeded timeout'])

    def test_unknown_fields_remain_fixed_or_null(self):
        for state in (None, [], {'Health': None}, {'Health': []}):
            self.assertEqual(fixture.health_record({'State': state}), {'status': 'unknown', 'checks': []})
        result = fixture.health_record({'State': {'Health': {'Status': 'private-name',
            'FailingStreak': True, 'Log': [{'Start': 'private-path', 'End': 'private-path',
                                         'ExitCode': 'private-code', 'Output': None}]}}})
        self.assertNotIn('private-', json.dumps(result))
        self.assertIsNone(result['failingStreak'])
        self.assertIsNone(result['checks'][0]['exitCode'])
        self.assertIsNone(result['checks'][0]['durationMs'])

    def test_only_four_bounded_checks_are_retained(self):
        value = {'State': {'Health': {'Log': [{'ExitCode': n} for n in range(12)]}}}
        self.assertEqual([r['exitCode'] for r in fixture.health_record(value)['checks']], [8, 9, 10, 11])


class EngineReads(unittest.TestCase):
    def serve(self, payload):
        paths = []
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                paths.append(self.path)
                self.send_response(200)
                self.send_header('Content-Length', str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            def log_message(self, *args):
                pass

        self.directory = tempfile.TemporaryDirectory(prefix='oc-hm-', dir='/tmp')
        self.addCleanup(self.directory.cleanup)
        path = (Path(self.directory.name) / 'engine.sock').resolve()
        server = socketserver.UnixStreamServer(str(path), Handler)
        self.addCleanup(server.server_close)
        thread = threading.Thread(target=server.serve_forever)
        thread.start()
        self.addCleanup(thread.join, 2)
        self.addCleanup(server.shutdown)
        return path, paths

    def test_engine_transport_reads_only_the_requested_unix_endpoint(self):
        path, paths = self.serve(b'{"State":{"Health":{"Status":"starting"}}}')
        status, value = fixture.request(path, '/containers/owned/json')
        self.assertEqual(status, 200)
        self.assertEqual(value['State']['Health']['Status'], 'starting')
        self.assertEqual(paths, ['/containers/owned/json'])

    def test_engine_response_limit_fails_closed(self):
        path, paths = self.serve(b' ' * ((2 << 20) + 1))
        with self.assertRaisesRegex(ValueError, 'ENGINE_RESPONSE_OR_IDENTITY_CHANGED'):
            fixture.request(path, '/containers/json?all=true')
        self.assertEqual(paths, ['/containers/json?all=true'])

    def test_observer_selects_the_owned_image_and_stops_after_its_removal(self):
        path, _ = self.serve(b'{}')
        image = 'sha256:' + 'a' * 64
        identifier = 'b' * 64
        replies = [(200, [{'Id': 'private-other', 'ImageID': 'other'}, {'Id': identifier, 'ImageID': image}]),
                   (200, {'Image': image, 'State': {'Running': True, 'Health': {'Status': 'starting'}}}),
                   (200, [])]
        output = io.StringIO()
        with patch.object(fixture, 'request', side_effect=replies) as query, patch.object(fixture.time, 'sleep'), redirect_stdout(output):
            fixture.observe(path, image)
        self.assertEqual([c.args[1] for c in query.call_args_list],
                         ['/containers/json?all=true', '/containers/' + identifier + '/json', '/containers/json?all=true'])
        row = json.loads(output.getvalue())
        self.assertEqual(row['role'], 'qualification')
        self.assertEqual(row['status'], 'starting')
        self.assertNotIn(identifier, output.getvalue())
        self.assertNotIn('private-other', output.getvalue())


if __name__ == '__main__':
    unittest.main()
