"""Filesystem ownership and own-child cleanup; no Docker or Broker is started."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import socket
import stat
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('with_broker', HERE / 'with-broker.py')
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class CommandEnvironment(unittest.TestCase):
    def test_environment_retains_toolchain_and_formal_assets_but_no_credentials(self):
        source = {'PATH': '/usr/bin', 'HOME': '/controlled/home',
                  'CARGO_HOME': '/controlled/cargo', 'RUSTUP_HOME': '/controlled/rustup',
                  'OPEN_COMPUTE_TEST_WORKERD': '/controlled/workerd',
                  'OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE': '/controlled/workerd.tar',
                  'RUSTC_WRAPPER': 'sccache', 'SCCACHE_GHA_ENABLED': 'true',
                  'SCCACHE_SERVER_PORT': '4226', 'LLVM_PROFILE_FILE': '/controlled/%m.profraw',
                  'CARGO_LLVM_COV': '1', 'RUSTFLAGS': '-C instrument-coverage',
                  'OPEN_COMPUTE_COVERAGE_RUN_DIR': '/controlled/coverage-run',
                  'NPM_TOKEN': 'controlled-test-data', 'GITHUB_TOKEN': 'controlled-test-data',
                  'CLOUDFLARE_API_TOKEN': 'controlled-test-data',
                  'OPEN_COMPUTE_ADMIN_TOKEN': 'controlled-test-data',
                  'DYLD_INSERT_LIBRARIES': '/controlled/not-loaded',
                  'MBX_GC_AUTO': 'true', 'MBX_TARGET_VIEWS': '1'}
        value = fixture.command_environment(Path('/ci/broker'), source)
        for key in ('PATH', 'HOME', 'CARGO_HOME', 'RUSTUP_HOME',
                    'OPEN_COMPUTE_TEST_WORKERD', 'OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE',
                    'RUSTC_WRAPPER', 'SCCACHE_GHA_ENABLED', 'SCCACHE_SERVER_PORT',
                    'LLVM_PROFILE_FILE', 'CARGO_LLVM_COV', 'RUSTFLAGS',
                    'OPEN_COMPUTE_COVERAGE_RUN_DIR'):
            self.assertEqual(value[key], source[key])
        for key in ('NPM_TOKEN', 'GITHUB_TOKEN', 'CLOUDFLARE_API_TOKEN',
                    'OPEN_COMPUTE_ADMIN_TOKEN', 'DYLD_INSERT_LIBRARIES'):
            self.assertNotIn(key, value)
        self.assertNotIn('MBX_GC_AUTO', value)
        self.assertNotIn('MBX_TARGET_VIEWS', value)


class Lifecycle(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(dir='/tmp')
        self.root = Path(self.temporary.name).resolve()
        self.run = self.root / 'run'
        self.run.mkdir(mode=0o700)
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.bind(str(self.root / 'engine.sock'))
        os.chmod(self.root / 'engine.sock', 0o660)
        self.values = {'socket': str(self.run / 'broker.sock'),
            'engine_socket': str(self.root / 'engine.sock'), 'peer_uid': os.getuid(),
            'sidecar_image': 'sha256:' + 'a' * 64, 'exec_helper_volume': 'controlled-helper',
            'exec_helper_sha256': 'b' * 64, 'qualification_image': 'sha256:' + 'c' * 64,
            'max_leases': 256, 'max_instances': 16, 'max_requests': 64,
            'relay_host': '127.0.0.1', 'spool_dir': str(self.run / 'spool'),
            'state_dir': str(self.run / 'state'), 'max_image_bytes': 2 << 30,
            'max_stream_bytes': 8 << 20,
            'resources': {'cpuNanos': 62500000, 'memoryBytes': 256 << 20,
                          'diskBytes': 2000000000, 'pids': 128, 'openFiles': 1024}}
        self.config = self.run / 'broker.toml'
        self.write_config()

    def tearDown(self):
        self.socket.close()
        self.temporary.cleanup()

    def write_config(self):
        text = '\n'.join(k + ' = ' + json.dumps(v) for k, v in self.values.items() if k != 'resources')
        text += '\n[resources]\n' + '\n'.join(k + ' = ' + str(v) for k, v in self.values['resources'].items())
        self.config.write_text(text + '\n')
        self.config.chmod(0o600)

    def test_owned_fresh_config_preserves_exact_resources(self):
        value, _ = fixture.owned_config(self.config)
        self.assertEqual(value, self.values)
        self.assertEqual(set(self.run.iterdir()), {self.config})

    def test_state_cannot_escape_owned_root(self):
        self.values['state_dir'] = str(self.root / 'foreign-state')
        self.write_config()
        with self.assertRaisesRegex(ValueError, '^OWNED_PATH_REQUIRED$'):
            fixture.owned_config(self.config)

    def test_previous_state_is_not_reused(self):
        (self.run / 'state').mkdir()
        with self.assertRaisesRegex(ValueError, '^FRESH_PRIVATE_ROOT_REQUIRED$'):
            fixture.owned_config(self.config)

    def test_world_readable_config_is_rejected(self):
        self.config.chmod(0o644)
        with self.assertRaisesRegex(ValueError, '^PRIVATE_CONFIG_REQUIRED$'):
            fixture.owned_config(self.config)

    def test_foreign_peer_uid_is_rejected(self):
        self.values['peer_uid'] += 1
        self.write_config()
        with self.assertRaisesRegex(ValueError, '^CLOSED_CONFIG_AND_PEER_UID_REQUIRED$'):
            fixture.owned_config(self.config)

    def test_world_writable_engine_socket_is_rejected(self):
        os.chmod(self.root / 'engine.sock', 0o666)
        with self.assertRaisesRegex(ValueError, '^ENGINE_SOCKET_AUTHORITY$'):
            fixture.owned_config(self.config)

    def test_clone_never_chmods_or_relinks_original(self):
        source = self.root / 'source'
        source.write_bytes(b'controlled executable bytes')
        source.chmod(0o755)
        linked = self.root / 'linked-source'
        os.link(source, linked)
        before = fixture.prepare.oci.identity(source.lstat())
        dest = self.run / 'ocd'
        fixture.clone_binary(source, hashlib.sha256(source.read_bytes()).hexdigest(), dest)
        self.assertEqual(before, fixture.prepare.oci.identity(source.lstat()))
        self.assertEqual(dest.read_bytes(), source.read_bytes())
        self.assertEqual(stat.S_IMODE(dest.stat().st_mode), 0o555)
        self.assertEqual(dest.stat().st_nlink, 1)
        self.assertNotEqual(dest.stat().st_ino, source.stat().st_ino)

    def test_wrong_binary_digest_stops_before_execution(self):
        source = self.root / 'source'
        source.write_bytes(b'controlled executable bytes')
        source.chmod(0o755)
        with self.assertRaisesRegex(ValueError, '^BINARY_CHANGED$'):
            fixture.clone_binary(source, '0' * 64, self.run / 'ocd')

    def test_live_own_child_is_terminated_and_reaped(self):
        process = subprocess.Popen([sys.executable, '-I', '-B', '-c', 'import time;time.sleep(60)'])
        try:
            value = fixture.stop_child(process)
            self.assertTrue(value['terminateSent'])
            self.assertFalse(value['killSent'])
            self.assertTrue(value['reaped'])
            self.assertIsNotNone(process.returncode)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()

    def test_completed_child_is_not_signalled(self):
        process = subprocess.Popen([sys.executable, '-I', '-B', '-c', 'pass'])
        process.wait()
        value = fixture.stop_child(process)
        self.assertFalse(value['terminateSent'])
        self.assertFalse(value['killSent'])
        self.assertEqual(value['exitCode'], 0)

    def test_timeout_branch_kills_only_the_given_child_then_reaps(self):
        calls = []

        class ControlledChild:
            pid = 123

            def poll(self):
                return None

            def terminate(self):
                calls.append('terminate')

            def kill(self):
                calls.append('kill')

            def wait(self, timeout):
                calls.append(('wait', timeout))
                if timeout == 30:
                    raise subprocess.TimeoutExpired('controlled-test-child', timeout)
                return -9

        value = fixture.stop_child(ControlledChild())
        self.assertEqual(calls, ['terminate', ('wait', 30), 'kill', ('wait', 5)])
        self.assertTrue(value['killSent'])
        self.assertTrue(value['reaped'])


if __name__ == '__main__':
    unittest.main()
