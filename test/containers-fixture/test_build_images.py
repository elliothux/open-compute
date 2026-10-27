"""Native image producer authority and owned-tag cleanup checks."""
import importlib.util
import hashlib
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('build_images', HERE / 'build-images.py')
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class BuildImages(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name).resolve()
        self.channel = socket.socket(socket.AF_UNIX)
        self.socket_path = self.root / 'engine.sock'
        self.channel.bind(str(self.socket_path))
        self.host = 'unix://' + str(self.socket_path)
        self.mbx = self.binary('mbx')
        self.sidecar = self.binary('sidecar')
        self.commands = []
        self.fail_revision = False
        self.fail_cleanup = False

    def tearDown(self):
        self.channel.close()
        self.temporary.cleanup()

    def binary(self, name, machine=183):
        header = bytearray(64)
        header[:6] = b'\x7fELF\x02\x01'
        header[18:20] = machine.to_bytes(2, 'little')
        target = self.root / name
        target.write_bytes(header)
        target.chmod(0o755)
        return target

    def command(self, args, **kwargs):
        env = kwargs['env']
        self.assertEqual(set(env), {'PATH', 'HOME', 'DOCKER_HOST', 'DOCKER_CONFIG'})
        self.assertEqual(env['DOCKER_HOST'], self.host)
        self.commands.append(args)
        if args[-1] == '--version':
            return subprocess.CompletedProcess(args, 0, 'mbx 1.22.0\n')
        if args[1] == 'info':
            return subprocess.CompletedProcess(args, 0, json.dumps({
                'OSType': 'linux', 'Architecture': 'aarch64'}))
        if args[1:3] == ['image', 'inspect']:
            return subprocess.CompletedProcess(args, 0,
                'sha256:' + hashlib.sha256(('engine-' + args[-1].rsplit(':', 1)[1]).encode()).hexdigest() + '\n')
        if self.fail_revision and 'REVISION=two' in args:
            raise subprocess.CalledProcessError(1, args)
        if args[1:3] == ['image', 'save']:
            Path(args[4]).write_bytes(b'controlled save')
        if self.fail_cleanup and args[1:3] == ['image', 'rm'] and args[-1].endswith(':two'):
            raise subprocess.CalledProcessError(1, args)
        return subprocess.CompletedProcess(args, 0, '')

    def export(self, source, destination, architecture, engine_image_id=None):
        self.assertEqual(source.read_bytes(), b'controlled save')
        self.assertEqual(architecture, 'arm64')
        if destination.name != 'qualification':
            self.assertEqual(engine_image_id, 'sha256:' +
                             hashlib.sha256(('engine-' + destination.name).encode()).hexdigest())
        destination.mkdir()
        return {'manifestSha256': destination.name,
                'configSha256': hashlib.sha256(destination.name.encode()).hexdigest(),
                'sourceSaveSha256': hashlib.sha256(source.read_bytes()).hexdigest()}

    def build(self, selected=None):
        with patch.object(fixture.platform, 'system', return_value='Linux'), \
                patch.object(fixture.platform, 'machine', return_value='aarch64'), \
                patch.object(fixture.shutil, 'which', return_value='/controlled/docker'), \
                patch.object(fixture.subprocess, 'run', side_effect=self.command), \
                patch.object(fixture.exporter, 'export', side_effect=self.export):
            return fixture.build(self.host, self.mbx, self.sidecar, self.root / 'output', selected)

    def test_native_binary_platform_and_identity(self):
        before = fixture.oci.identity(self.mbx.lstat())
        digest = fixture.copy_elf(self.mbx, self.root / 'copy', 'arm64')
        self.assertEqual(len(digest), 64)
        self.assertEqual(before, fixture.oci.identity(self.mbx.lstat()))
        with self.assertRaisesRegex(ValueError, '^BUILD_BINARY_PLATFORM$'):
            fixture.copy_elf(self.mbx, self.root / 'wrong', 'amd64')

    def test_symlink_binary_is_rejected(self):
        link = self.root / 'link'
        link.symlink_to(self.mbx)
        with self.assertRaisesRegex(ValueError, '^BUILD_BINARY_PATH$'):
            fixture.copy_elf(link, self.root / 'copy', 'arm64')

    def test_two_revisions_and_separate_helper_free_qualification_image(self):
        result = self.build()
        self.assertEqual(set(result['images']), {'one', 'two', 'sidecar', 'qualification'})
        builds = [args for args in self.commands if args[1] == 'build']
        self.assertEqual(len(builds), 4)
        self.assertTrue(all('linux/arm64' in args for args in builds))
        for name in ('sidecar', 'one', 'two'):
            engine_id = 'sha256:' + hashlib.sha256(('engine-' + name).encode()).hexdigest()
            self.assertTrue(any(args[1:3] == ['image', 'save'] and args[-1] == engine_id
                                for args in self.commands))
            self.assertEqual(result['engineImageIds'][name],
                             'sha256:' + result['images'][name]['configSha256'])
        self.assertEqual([args[-1].rsplit(':', 1)[1] for args in self.commands
                          if args[1:3] == ['image', 'rm']], ['two', 'one', 'sidecar', 'base'])
        qualification = [args for args in self.commands if args[1:3] == ['image', 'save']][-1]
        self.assertEqual(qualification[-1], fixture.ALPINE)
        self.assertFalse(list((self.root / 'output').glob('build-*')))
        self.assertTrue((self.root / 'output/images.json').is_file())
        self.assertEqual(set(result['dockerArchives']), {'sidecar', 'qualification'})
        for name, archive in result['dockerArchives'].items():
            path = self.root / 'output' / archive['path']
            self.assertEqual(path.read_bytes(), b'controlled save')
            self.assertEqual(archive['sha256'], result['images'][name]['sourceSaveSha256'])
            self.assertEqual(archive['bytes'], path.stat().st_size)

    def test_build_failure_cleans_only_already_built_owned_tags(self):
        self.fail_revision = True
        with self.assertRaises(subprocess.CalledProcessError):
            self.build()
        removed = [args[-1].rsplit(':', 1)[1] for args in self.commands if args[1:3] == ['image', 'rm']]
        self.assertEqual(removed, ['one', 'sidecar', 'base'])
        self.assertFalse((self.root / 'output/images.json').exists())

    def test_cleanup_failure_still_attempts_all_owned_tags_and_does_not_publish(self):
        self.fail_cleanup = True
        with self.assertRaisesRegex(ValueError, '^BUILD_TAG_CLEANUP_FAILED$'):
            self.build()
        self.assertEqual(len([args for args in self.commands if args[1:3] == ['image', 'rm']]), 4)
        self.assertFalse((self.root / 'output/images.json').exists())

    def test_remote_engine_endpoint_is_rejected(self):
        with patch.object(fixture.platform, 'system', return_value='Linux'), \
                patch.object(fixture.platform, 'machine', return_value='aarch64'):
            with self.assertRaisesRegex(ValueError, '^BUILD_EXPLICIT_DOCKER_SOCKET$'):
                fixture.build('tcp://127.0.0.1:2375', self.mbx, self.sidecar, self.root / 'output')

    def test_failed_second_artifact_retry_does_not_rebuild_passed_artifacts(self):
        result = self.build('two')
        self.assertEqual(set(result['images']), {'two'})
        self.assertEqual(set(result['engineImageIds']), {'base', 'two'})
        self.assertEqual(result['dockerArchives'], {})
        self.assertEqual(len([args for args in self.commands if args[1] == 'build']), 2)
        self.assertFalse(any(args[1] == 'pull' for args in self.commands))

    def test_qualification_retry_has_no_image_build_or_owned_tag(self):
        result = self.build('qualification')
        self.assertEqual(set(result['images']), {'qualification'})
        self.assertEqual(result['engineImageIds'], {})
        self.assertEqual(set(result['dockerArchives']), {'qualification'})
        self.assertFalse(any(args[1] == 'build' or args[1:3] == ['image', 'rm']
                             for args in self.commands))

    def test_sidecar_retry_does_not_build_the_rust_fixture(self):
        result = self.build('sidecar')
        self.assertEqual(set(result['images']), {'sidecar'})
        self.assertEqual(set(result['engineImageIds']), {'sidecar'})
        self.assertEqual(len([args for args in self.commands if args[1] == 'build']), 1)

    def test_unknown_selected_artifact_is_rejected_before_mutation(self):
        with self.assertRaisesRegex(ValueError, '^BUILD_SELECTED_IMAGE$'):
            self.build('unknown')
        self.assertFalse((self.root / 'output').exists())
        self.assertEqual(self.commands, [])


if __name__ == '__main__':
    unittest.main()
