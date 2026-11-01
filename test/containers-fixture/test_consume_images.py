"""Synthetic artifact parsing checks; these never qualify a Broker or CI runner."""
import copy
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import shutil
import tarfile
import tempfile
import unittest

import test_export_image as export_tests

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('consume_images', HERE / 'consume-images.py')
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class Consume(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name).resolve()
        self.report = {'schemaVersion': 1, 'architecture': 'arm64', 'selectedImage': None,
            'binaryInputs': {'sidecarSha256': 'b' * 64},
            'sourceInputs': {}, 'engineImageIds': {'base': 'sha256:' + 'c' * 64},
            'images': {}, 'dockerArchives': {}}
        for name in fixture.producer.SOURCE_FILES:
            digest, size, _ = fixture.oci.regular(fixture.producer.ROOT / name, 1 << 20)
            self.report['sourceInputs'][name] = {'sha256': digest, 'bytes': size}
        for name in fixture.producer.IMAGES:
            maker = export_tests.Export('test_raw_layer_compression_is_reproducible')
            maker.setUp()
            try:
                stream = io.BytesIO()
                with tarfile.open(fileobj=stream, mode='w') as archive:
                    data = name.encode()
                    entry = tarfile.TarInfo('controlled/data')
                    entry.size = len(data)
                    archive.addfile(entry, io.BytesIO(data))
                maker.layer = stream.getvalue()
                maker.save()
                value = fixture.producer.exporter.export(maker.source, self.root / name, 'arm64')
                self.report['images'][name] = value
                if name != 'qualification':
                    self.report['engineImageIds'][name] = 'sha256:' + value['configSha256']
                if name in ('sidecar', 'qualification'):
                    path = self.root / (name + '.docker.tar')
                    shutil.copyfile(maker.source, path)
                    self.report['dockerArchives'][name] = {'path': path.name,
                        'sha256': value['sourceSaveSha256'], 'bytes': path.stat().st_size}
            finally:
                maker.tearDown()
        self.write()

    def tearDown(self):
        self.temporary.cleanup()

    def write(self):
        (self.root / 'images.json').write_text(json.dumps(self.report))

    def test_complete_artifact_emits_only_two_images_without_runtime_authority(self):
        result = fixture.consume(self.root, 'arm64')
        self.assertEqual(set(result), {'images'})
        self.assertEqual(len(result['images']), 2)
        self.assertNotEqual(Path(result['images'][0]['manifest']).name,
                            Path(result['images'][1]['manifest']).name)
        self.assertNotIn('containers', result)

    def test_partial_producer_report_is_not_complete(self):
        self.report['selectedImage'] = 'two'
        self.write()
        with self.assertRaisesRegex(ValueError, '^COMPLETE_PRODUCER_REPORT_REQUIRED$'):
            fixture.consume(self.root, 'arm64')

    def test_binary_inputs_require_only_the_verified_sidecar_digest(self):
        for binaries in ({}, {'sidecarSha256': 'invalid'},
                         {'sidecarSha256': 'b' * 64, 'mbxSha256': 'a' * 64}):
            self.report['binaryInputs'] = binaries
            self.write()
            with self.assertRaisesRegex(ValueError, '^ARTIFACT_BINARY_FIELDS$'):
                fixture.consume(self.root, 'arm64')

    def test_source_mismatch_is_rejected(self):
        self.report['sourceInputs'][fixture.producer.SOURCE_FILES[0]]['sha256'] = '0' * 64
        self.write()
        with self.assertRaisesRegex(ValueError, '^ARTIFACT_SOURCE_MISMATCH$'):
            fixture.consume(self.root, 'arm64')

    def test_raw_archive_tamper_is_rejected(self):
        with (self.root / 'sidecar.docker.tar').open('ab') as stream:
            stream.write(b'controlled change')
        with self.assertRaisesRegex(ValueError, '^ARTIFACT_ARCHIVE_MISMATCH$'):
            fixture.consume(self.root, 'arm64')

    def test_self_consistent_archive_hash_cannot_substitute_another_image(self):
        data = (self.root / 'qualification.docker.tar').read_bytes()
        (self.root / 'sidecar.docker.tar').write_bytes(data)
        digest = hashlib.sha256(data).hexdigest()
        self.report['dockerArchives']['sidecar'].update(sha256=digest, bytes=len(data))
        self.report['images']['sidecar']['sourceSaveSha256'] = digest
        self.write()
        with self.assertRaisesRegex(ValueError, '^ARTIFACT_BOOTSTRAP_GRAPH_MISMATCH$'):
            fixture.consume(self.root, 'arm64')

    def test_archive_path_cannot_escape_the_artifact(self):
        self.report['dockerArchives']['sidecar']['path'] = '../outside'
        self.write()
        with self.assertRaisesRegex(ValueError, '^ARTIFACT_ARCHIVE_PATH$'):
            fixture.consume(self.root, 'arm64')

    def test_missing_bootstrap_archive_metadata_is_rejected(self):
        self.report['dockerArchives'].pop('qualification')
        self.write()
        with self.assertRaisesRegex(ValueError, '^COMPLETE_IMAGE_AND_BOOTSTRAP_ARCHIVES_REQUIRED$'):
            fixture.consume(self.root, 'arm64')

    def test_blob_tamper_is_rejected(self):
        manifest, _, _ = fixture.oci.document(self.root / 'one/manifest.json')
        layer = self.root / 'one/blobs/sha256' / manifest['layers'][0]['digest'][7:]
        layer.write_bytes(b'controlled wrong layer')
        with self.assertRaisesRegex(ValueError, '^OCI_LAYER_DIGEST_OR_SIZE$'):
            fixture.consume(self.root, 'arm64')

    def test_copied_image_does_not_satisfy_two_distinct_revisions(self):
        shutil.rmtree(self.root / 'two')
        shutil.copytree(self.root / 'one', self.root / 'two')
        self.report['images']['two'] = copy.deepcopy(self.report['images']['one'])
        self.report['engineImageIds']['two'] = self.report['engineImageIds']['one']
        self.write()
        with self.assertRaisesRegex(ValueError, '^DISTINCT_IMAGE_INPUTS_REQUIRED$'):
            fixture.consume(self.root, 'arm64')

    def test_wrong_architecture_and_boolean_schema_version_are_rejected(self):
        with self.assertRaisesRegex(ValueError, '^ARTIFACT_ARCHITECTURE$'):
            fixture.consume(self.root, 'unsupported')
        with self.assertRaisesRegex(ValueError, '^COMPLETE_PRODUCER_REPORT_REQUIRED$'):
            fixture.consume(self.root, 'amd64')
        self.report['schemaVersion'] = True
        self.write()
        with self.assertRaisesRegex(ValueError, '^COMPLETE_PRODUCER_REPORT_REQUIRED$'):
            fixture.consume(self.root, 'arm64')

    def test_duplicate_report_fields_are_rejected(self):
        raw = (self.root / 'images.json').read_text()
        (self.root / 'images.json').write_text('{"schemaVersion":1,' + raw[1:])
        with self.assertRaisesRegex(ValueError, '^JSON_DUPLICATE_FIELD$'):
            fixture.consume(self.root, 'arm64')


if __name__ == '__main__':
    unittest.main()
