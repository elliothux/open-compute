"""Bounded Docker-save conversion and digest failure cases."""
import gzip
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('export_image', HERE / 'export-image.py')
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class Export(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name).resolve()
        self.source = self.root / 'image.tar'
        self.destination = self.root / 'oci'
        stream = io.BytesIO()
        with tarfile.open(fileobj=stream, mode='w') as archive:
            value = b'controlled container bytes'
            entry = tarfile.TarInfo('app/data')
            entry.size = len(value)
            archive.addfile(entry, io.BytesIO(value))
        self.layer = stream.getvalue()

    def tearDown(self):
        self.temporary.cleanup()

    def save(self, compressed=False, bad_diff=False, extra=None, duplicate=False, architecture='arm64'):
        diff = 'sha256:' + ('0' * 64 if bad_diff else hashlib.sha256(self.layer).hexdigest())
        config = json.dumps({'os': 'linux', 'architecture': architecture,
                             'rootfs': {'type': 'layers', 'diff_ids': [diff]}}).encode()
        name = hashlib.sha256(config).hexdigest() + '.json'
        manifest = json.dumps([{'Config': name, 'RepoTags': ['controlled:fixture'],
                                'Layers': ['layer.tar']}]).encode()
        with tarfile.open(self.source, 'w') as archive:
            rows = [('manifest.json', manifest), (name, config),
                    ('layer.tar', gzip.compress(self.layer, mtime=0) if compressed else self.layer)]
            if extra:
                rows.append(extra)
            if duplicate:
                rows.append(rows[0])
            for path, data in rows:
                entry = tarfile.TarInfo(path)
                entry.size = len(data)
                archive.addfile(entry, io.BytesIO(data))

    def save_engine_identity(self, indexed=False, bad_config=False, architecture='arm64'):
        self.save()
        with tarfile.open(self.source) as archive:
            item = json.load(archive.extractfile('manifest.json'))[0]
            config = archive.extractfile(item['Config']).read()
        config_sha = hashlib.sha256(config).hexdigest()
        manifest = fixture.encode({'schemaVersion': 2, 'mediaType': fixture.oci.MANIFEST,
            'config': {'mediaType': fixture.oci.CONFIG, 'size': len(config),
                       'digest': 'sha256:' + ('0' * 64 if bad_config else config_sha)},
            'layers': []})
        digest = hashlib.sha256(manifest).hexdigest()
        rows = [('blobs/sha256/' + digest, manifest)]
        if indexed:
            index = fixture.encode({'schemaVersion': 2,
                'mediaType': 'application/vnd.oci.image.index.v1+json', 'manifests': [{
                    'mediaType': fixture.oci.MANIFEST, 'digest': 'sha256:' + digest,
                    'size': len(manifest), 'platform': {'os': 'linux',
                                                       'architecture': architecture}}]})
            digest = hashlib.sha256(index).hexdigest()
            rows.append(('blobs/sha256/' + digest, index))
        with tarfile.open(self.source, 'a') as archive:
            for name, raw in rows:
                entry = tarfile.TarInfo(name)
                entry.size = len(raw)
                archive.addfile(entry, io.BytesIO(raw))
        return 'sha256:' + digest, config_sha

    def test_engine_config_manifest_and_index_bind_to_exported_config(self):
        for kind in ('config', 'manifest', 'index'):
            with self.subTest(kind=kind):
                image_id, config_sha = self.save_engine_identity(indexed=kind == 'index')
                if kind == 'config':
                    image_id = 'sha256:' + config_sha
                result = fixture.export(self.source, self.root / kind, 'arm64', image_id)
                self.assertEqual(result['configSha256'], config_sha)

    def test_engine_manifest_cannot_select_another_config(self):
        for indexed in (False, True):
            with self.subTest(indexed=indexed):
                image_id, _ = self.save_engine_identity(indexed=indexed, bad_config=True)
                with self.assertRaisesRegex(ValueError, '^SAVE_ENGINE_CONFIG_MISMATCH$'):
                    fixture.export(self.source, self.destination, 'arm64', image_id)
                self.assertFalse(self.destination.exists())

    def test_engine_index_cannot_select_another_platform(self):
        image_id, _ = self.save_engine_identity(indexed=True, architecture='amd64')
        with self.assertRaisesRegex(ValueError, '^SAVE_ENGINE_PLATFORM_MANIFEST$'):
            fixture.export(self.source, self.destination, 'arm64', image_id)
        self.assertFalse(self.destination.exists())

    def test_raw_and_gzip_layers_preserve_diff_id_and_source_identity(self):
        for compressed in (False, True):
            with self.subTest(compressed=compressed):
                self.save(compressed=compressed)
                before = fixture.oci.identity(self.source.lstat())
                dest = self.root / str(compressed)
                result = fixture.export(self.source, dest, 'arm64')
                self.assertEqual(result['layers'], 1)
                self.assertEqual(result['unpackedBytes'], len(self.layer))
                self.assertEqual(before, fixture.oci.identity(self.source.lstat()))
                self.assertTrue(fixture.oci.image(dest, 'arm64'))
                manifest = json.loads((dest / 'manifest.json').read_bytes())
                self.assertEqual(manifest['layers'][0]['mediaType'],
                                 'application/vnd.oci.image.layer.v1.tar+gzip')

    def test_raw_layer_compression_is_reproducible(self):
        self.save()
        first = fixture.export(self.source, self.root / 'first', 'arm64')
        second = fixture.export(self.source, self.root / 'second', 'arm64')
        self.assertEqual(first, second)

    def test_wrong_diff_id_is_rejected_without_publishing_index(self):
        self.save(bad_diff=True)
        with self.assertRaisesRegex(ValueError, '^SAVE_DIFF_ID$'):
            fixture.export(self.source, self.destination, 'arm64')
        self.assertFalse((self.destination / 'index.json').exists())

    def test_duplicate_archive_member_is_rejected(self):
        self.save(duplicate=True)
        with self.assertRaisesRegex(ValueError, '^SAVE_ENTRY_TYPE_OR_DUPLICATE$'):
            fixture.export(self.source, self.destination, 'arm64')

    def test_escape_and_absolute_archive_names_are_rejected(self):
        for name in ('../outside', '/absolute', 'a/../outside', 'a\\outside'):
            with self.subTest(name=name):
                self.save(extra=(name, b'controlled'))
                with self.assertRaisesRegex(ValueError, '^SAVE_ENTRY_PATH$'):
                    fixture.export(self.source, self.destination, 'arm64')

    def test_wrong_platform_is_rejected(self):
        self.save(architecture='amd64')
        with self.assertRaisesRegex(ValueError, '^SAVE_IMAGE_PLATFORM$'):
            fixture.export(self.source, self.destination, 'arm64')

    def test_existing_output_is_not_reused(self):
        self.save()
        self.destination.mkdir()
        with self.assertRaisesRegex(ValueError, '^SAVE_CANONICAL_AND_FRESH_PATHS$'):
            fixture.export(self.source, self.destination, 'arm64')

    def test_unpacked_limit_applies_to_gzip(self):
        stream = io.BytesIO()
        with tarfile.open(fileobj=stream, mode='w') as archive:
            data = b'x' * (128 << 10)
            entry = tarfile.TarInfo('app/data')
            entry.size = len(data)
            archive.addfile(entry, io.BytesIO(data))
        self.layer = stream.getvalue()
        self.save(compressed=True)
        old = fixture.LIMIT
        try:
            fixture.LIMIT = 32 << 10
            self.assertLessEqual(self.source.stat().st_size, fixture.LIMIT)
            with self.assertRaisesRegex(ValueError, '^SAVE_UNPACKED_LIMIT$'):
                fixture.export(self.source, self.destination, 'arm64')
        finally:
            fixture.LIMIT = old

    def test_link_member_is_rejected_without_extraction(self):
        self.save()
        with tarfile.open(self.source, 'a') as archive:
            entry = tarfile.TarInfo('link')
            entry.type = tarfile.SYMTYPE
            entry.linkname = '../outside'
            archive.addfile(entry)
        with self.assertRaisesRegex(ValueError, '^SAVE_ENTRY_TYPE_OR_DUPLICATE$'):
            fixture.export(self.source, self.destination, 'arm64')
        self.assertFalse(self.destination.exists())


if __name__ == '__main__':
    unittest.main()
