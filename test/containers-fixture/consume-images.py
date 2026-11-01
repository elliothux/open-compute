#!/usr/bin/env python3
"""Verify a complete producer artifact and emit image-only build inputs."""
import argparse
import importlib.util
import json
from pathlib import Path
import re
import sys
import tempfile

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('image_producer', HERE / 'build-images.py')
producer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(producer)
oci = producer.oci
REPORT_FIELDS = {'schemaVersion', 'architecture', 'selectedImage', 'binaryInputs',
                 'sourceInputs', 'engineImageIds', 'images', 'dockerArchives'}
IMAGE_FIELDS = {'sourceSaveSha256', 'manifestSha256', 'configSha256', 'architecture',
                'layers', 'packedBytes', 'unpackedBytes'}


def digest(value):
    return isinstance(value, str) and re.fullmatch('[0-9a-f]{64}', value) is not None


def consume(directory, architecture):
    oci.require(architecture in ('arm64', 'amd64'), 'ARTIFACT_ARCHITECTURE')
    oci.require(directory.is_absolute() and directory.resolve() == directory
                and directory.is_dir(), 'ARTIFACT_DIRECTORY')
    _, _, raw = oci.regular(directory / 'images.json', 65536, True)
    report = json.loads(raw, object_pairs_hook=oci.pairs)
    oci.require(isinstance(report, dict) and set(report) == REPORT_FIELDS
                and type(report['schemaVersion']) is int and report['schemaVersion'] == 1
                and report['architecture'] == architecture and report['selectedImage'] is None,
                'COMPLETE_PRODUCER_REPORT_REQUIRED')
    sources = report['sourceInputs']
    oci.require(isinstance(sources, dict) and set(sources) == set(producer.SOURCE_FILES),
                'ARTIFACT_SOURCE_FIELDS')
    for name, expected in sources.items():
        actual, size, _ = oci.regular(producer.ROOT / name, 1 << 20)
        oci.require(isinstance(expected, dict) and expected == {'sha256': actual, 'bytes': size}
                    and type(expected['bytes']) is int, 'ARTIFACT_SOURCE_MISMATCH')
    binaries = report['binaryInputs']
    oci.require(isinstance(binaries, dict) and set(binaries) == {'sidecarSha256'}
                and all(digest(value) for value in binaries.values()), 'ARTIFACT_BINARY_FIELDS')
    identities = report['engineImageIds']
    oci.require(isinstance(identities, dict) and set(identities) == {'base', 'one', 'two', 'sidecar'}
                and all(isinstance(value, str) and value.startswith('sha256:')
                        and digest(value[7:]) for value in identities.values()), 'ARTIFACT_IMAGE_IDS')
    metadata = report['images']
    archives = report['dockerArchives']
    oci.require(isinstance(metadata, dict) and set(metadata) == set(producer.IMAGES)
                and isinstance(archives, dict) and set(archives) == {'sidecar', 'qualification'},
                'COMPLETE_IMAGE_AND_BOOTSTRAP_ARCHIVES_REQUIRED')
    apps = oci.images([directory / 'one', directory / 'two'], architecture)
    graphs = {'one': apps[0], 'two': apps[1], **{
        name: oci.image(directory / name, architecture) for name in ('sidecar', 'qualification')}}
    for name, graph in graphs.items():
        value = metadata[name]
        oci.require(isinstance(value, dict) and set(value) == IMAGE_FIELDS
                    and all(digest(value[key]) for key in (
                        'sourceSaveSha256', 'manifestSha256', 'configSha256'))
                    and value['architecture'] == architecture, 'ARTIFACT_IMAGE_FIELDS')
        manifest, manifest_sha, _ = oci.document(Path(graph['manifest']))
        oci.require(manifest_sha == value['manifestSha256']
                    and manifest['config']['digest'] == 'sha256:' + value['configSha256']
                    and type(value['layers']) is int and value['layers'] == len(manifest['layers'])
                    and type(value['packedBytes']) is int
                    and value['packedBytes'] == sum(layer['size'] for layer in manifest['layers'])
                    and type(value['unpackedBytes']) is int and 0 < value['unpackedBytes'] <= 2 << 30,
                    'ARTIFACT_IMAGE_GRAPH_MISMATCH')
        if name != 'qualification':
            oci.require(identities[name] == 'sha256:' + value['configSha256'], 'ARTIFACT_IMAGE_ID_MISMATCH')
        if name in archives:
            archive = archives[name]
            oci.require(isinstance(archive, dict) and set(archive) == {'path', 'sha256', 'bytes'}
                        and archive['path'] == name + '.docker.tar', 'ARTIFACT_ARCHIVE_PATH')
            actual, size, _ = oci.regular(directory / archive['path'], 2 << 30)
            oci.require(archive['sha256'] == actual == value['sourceSaveSha256']
                        and type(archive['bytes']) is int and archive['bytes'] == size,
                        'ARTIFACT_ARCHIVE_MISMATCH')
            temporary = producer.ROOT / '.temp'
            temporary.mkdir(exist_ok=True)
            oci.require(temporary.resolve() == temporary and temporary.is_dir(), 'ARTIFACT_VERIFY_ROOT')
            with tempfile.TemporaryDirectory(prefix='p23-artifact-verify-', dir=temporary) as work:
                exported = producer.exporter.export(directory / archive['path'], Path(work) / name,
                                                     architecture)
                oci.require(exported == value, 'ARTIFACT_BOOTSTRAP_GRAPH_MISMATCH')
    # Full runtime inputs require prepare-input.py's separate live Broker handshake.
    return {'images': apps}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', required=True, type=Path)
    parser.add_argument('--architecture', required=True, choices=('arm64', 'amd64'))
    args = parser.parse_args()
    print(json.dumps(consume(args.directory, args.architecture), separators=(',', ':')))


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        code = str(error) if isinstance(error, ValueError) else ''
        if re.fullmatch('[A-Z_]+', code) is None:
            code = type(error).__name__
        print('P23_ARTIFACT_CONSUMPTION_FAILED: ' + code, file=sys.stderr)
        raise SystemExit(1)
