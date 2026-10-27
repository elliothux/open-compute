#!/usr/bin/env python3
"""Convert one bounded Docker-save archive into a verified OCI image layout."""
import argparse
import gzip
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import tarfile

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('oci_images', HERE / 'oci-images.py')
oci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(oci)
LIMIT = 2 << 30
METADATA_LIMIT = 1 << 20


def encode(value):
    return json.dumps(value, separators=(',', ':')).encode()


def relative_name(name):
    oci.require(isinstance(name, str) and 0 < len(name.encode()) <= 4096
                and '\\' not in name and not any(ord(c) < 32 for c in name),
                'SAVE_ENTRY_PATH')
    path = PurePosixPath(name.rstrip('/'))
    oci.require(not path.is_absolute() and path.parts
                and all(part not in ('.', '..') for part in path.parts)
                and str(path) == name.rstrip('/'), 'SAVE_ENTRY_PATH')
    return str(path)


def members(archive):
    result = {}
    for entry in archive:
        oci.require(len(result) < 4096, 'SAVE_ENTRY_COUNT')
        name = relative_name(entry.name)
        oci.require(name not in result and (entry.isreg() or entry.isdir())
                    and not entry.issparse() and 0 <= entry.size <= LIMIT,
                    'SAVE_ENTRY_TYPE_OR_DUPLICATE')
        result[name] = entry
    return result


def regular(entries, name):
    name = relative_name(name)
    entry = entries.get(name)
    oci.require(entry is not None and entry.isreg(), 'SAVE_REGULAR_REFERENCE')
    return entry


def metadata(archive, entries, name):
    entry = regular(entries, name)
    oci.require(0 < entry.size <= METADATA_LIMIT, 'SAVE_METADATA_LIMIT')
    with archive.extractfile(entry) as stream:
        raw = stream.read(METADATA_LIMIT + 1)
    oci.require(len(raw) == entry.size, 'SAVE_METADATA_TRUNCATED')
    return json.loads(raw, object_pairs_hook=oci.pairs), raw


def put(blobs, data):
    digest = hashlib.sha256(data).hexdigest()
    with (blobs / digest).open('xb') as output:
        output.write(data)
    return digest, len(data)


def layer(archive, entry, expected, destination, remaining):
    oci.require(0 < entry.size <= remaining, 'SAVE_LAYER_LIMIT')
    pending = destination / 'layer.pending'
    digest = hashlib.sha256()
    count = 0
    with archive.extractfile(entry) as stream, pending.open('xb') as output:
        while chunk := stream.read(1 << 20):
            count += len(chunk)
            oci.require(count <= entry.size, 'SAVE_LAYER_GREW')
            digest.update(chunk)
            output.write(chunk)
    oci.require(count == entry.size, 'SAVE_LAYER_TRUNCATED')
    with pending.open('rb') as stream:
        magic = stream.read(4)
    oci.require(magic != b'\x28\xb5\x2f\xfd', 'SAVE_LAYER_COMPRESSION')
    compressed = magic.startswith(b'\x1f\x8b')
    diff = hashlib.sha256()
    unpacked = 0
    with (gzip.open(pending, 'rb') if compressed else pending.open('rb')) as stream:
        while chunk := stream.read(1 << 20):
            unpacked += len(chunk)
            oci.require(unpacked <= remaining, 'SAVE_UNPACKED_LIMIT')
            diff.update(chunk)
    oci.require('sha256:' + diff.hexdigest() == expected, 'SAVE_DIFF_ID')
    if not compressed:
        packed = destination / 'layer.gzip.pending'
        with pending.open('rb') as source, packed.open('xb') as output:
            with gzip.GzipFile(filename='', fileobj=output, mode='wb', compresslevel=6, mtime=0) as encoded:
                shutil.copyfileobj(source, encoded, 1 << 20)
        pending.unlink()
        packed.rename(pending)
        with pending.open('rb') as source:
            digest = hashlib.file_digest(source, 'sha256')
        count = pending.stat().st_size
    name = digest.hexdigest()
    target = destination / 'blobs' / 'sha256' / name
    if target.exists():
        oci.require(oci.regular(target, LIMIT)[0] == name, 'SAVE_BLOB_COLLISION')
        pending.unlink()
    else:
        pending.rename(target)
    return {'mediaType': 'application/vnd.oci.image.layer.v1.tar+gzip',
            'digest': 'sha256:' + name, 'size': count}, unpacked


def verify_engine_identity(archive, entries, image_id, config_sha, architecture):
    oci.require(isinstance(image_id, str)
                and re.fullmatch('sha256:[0-9a-f]{64}', image_id), 'SAVE_ENGINE_IMAGE_ID')
    if image_id == 'sha256:' + config_sha:
        return
    value, raw = metadata(archive, entries, 'blobs/sha256/' + image_id[7:])
    oci.require(hashlib.sha256(raw).hexdigest() == image_id[7:], 'SAVE_ENGINE_IMAGE_DIGEST')
    if value.get('mediaType') == 'application/vnd.oci.image.index.v1+json':
        manifests = value.get('manifests')
        oci.require(isinstance(manifests, list) and len(manifests) <= 256,
                    'SAVE_ENGINE_INDEX')
        selected = [row for row in manifests if isinstance(row, dict)
                    and isinstance(row.get('platform'), dict)
                    and row['platform'].get('os') == 'linux'
                    and row['platform'].get('architecture') == architecture]
        oci.require(len(selected) == 1, 'SAVE_ENGINE_PLATFORM_MANIFEST')
        digest, size = oci.descriptor(selected[0], {oci.MANIFEST,
            'application/vnd.docker.distribution.manifest.v2+json'})
        value, raw = metadata(archive, entries, 'blobs/sha256/' + digest)
        oci.require(hashlib.sha256(raw).hexdigest() == digest and len(raw) == size,
                    'SAVE_ENGINE_MANIFEST_DIGEST')
    oci.require(value.get('schemaVersion') == 2 and value.get('mediaType') in {
        oci.MANIFEST, 'application/vnd.docker.distribution.manifest.v2+json'},
        'SAVE_ENGINE_MANIFEST_SCHEMA')
    config = value.get('config')
    oci.require(isinstance(config, dict) and config.get('digest') == 'sha256:' + config_sha,
                'SAVE_ENGINE_CONFIG_MISMATCH')


def export(source, destination, architecture, engine_image_id=None):
    oci.require(source.is_absolute() and source.resolve() == source
                and destination.is_absolute() and destination.parent.resolve() == destination.parent
                and not os.path.lexists(destination), 'SAVE_CANONICAL_AND_FRESH_PATHS')
    descriptor = os.open(source, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    with os.fdopen(descriptor, 'rb') as file:
        before = os.fstat(file.fileno())
        oci.require(stat.S_ISREG(before.st_mode) and 0 < before.st_size <= LIMIT,
                    'SAVE_SOURCE_LIMIT')
        source_sha = hashlib.file_digest(file, 'sha256').hexdigest()
        file.seek(0)
        with tarfile.open(fileobj=file, mode='r:') as archive:
            entries = members(archive)
            manifest, _ = metadata(archive, entries, 'manifest.json')
            oci.require(isinstance(manifest, list) and len(manifest) == 1
                        and isinstance(manifest[0], dict), 'SAVE_SINGLE_IMAGE')
            item = manifest[0]
            oci.require(set(item) <= {'Config', 'RepoTags', 'Layers', 'LayerSources'}
                        and {'Config', 'Layers'} <= set(item), 'SAVE_MANIFEST_FIELDS')
            config, config_raw = metadata(archive, entries, item['Config'])
            oci.require(isinstance(config, dict) and config.get('os') == 'linux'
                        and config.get('architecture') == architecture, 'SAVE_IMAGE_PLATFORM')
            rootfs = config.get('rootfs')
            oci.require(isinstance(rootfs, dict) and rootfs.get('type') == 'layers',
                        'SAVE_ROOTFS')
            layers, diffs = item['Layers'], rootfs.get('diff_ids')
            oci.require(isinstance(layers, list) and isinstance(diffs, list)
                        and 0 < len(layers) <= 256 and len(layers) == len(diffs)
                        and all(isinstance(value, str) and re.fullmatch('sha256:[0-9a-f]{64}', value)
                                for value in diffs), 'SAVE_LAYER_GRAPH')
            config_sha = hashlib.sha256(config_raw).hexdigest()
            oci.require(PurePosixPath(item['Config']).name in (config_sha, config_sha + '.json'),
                        'SAVE_CONFIG_ADDRESS')
            if engine_image_id is not None:
                verify_engine_identity(archive, entries, engine_image_id, config_sha, architecture)
            destination.mkdir(mode=0o700)
            blobs = destination / 'blobs' / 'sha256'
            blobs.mkdir(parents=True, mode=0o700)
            put(blobs, config_raw)
            selected = []
            unpacked = packed = 0
            for name, expected in zip(layers, diffs, strict=True):
                entry = regular(entries, name)
                value, size = layer(archive, entry, expected, destination, LIMIT - unpacked)
                selected.append(value)
                unpacked += size
                packed += value['size']
                oci.require(packed <= LIMIT, 'SAVE_PACKED_LIMIT')
            file.seek(0)
            oci.require(hashlib.file_digest(file, 'sha256').hexdigest() == source_sha
                        and oci.identity(before) == oci.identity(os.fstat(file.fileno()))
                        and oci.identity(before) == oci.identity(source.lstat()), 'SAVE_SOURCE_CHANGED')
            value = {'schemaVersion': 2, 'mediaType': oci.MANIFEST,
                     'config': {'mediaType': oci.CONFIG, 'digest': 'sha256:' + config_sha,
                                'size': len(config_raw)}, 'layers': selected}
            raw = encode(value)
            digest, size = put(blobs, raw)
            (destination / 'manifest.json').write_bytes(raw)
            (destination / 'oci-layout').write_bytes(encode({'imageLayoutVersion': '1.0.0'}))
            (destination / 'index.json').write_bytes(encode({'schemaVersion': 2, 'manifests': [{
                'mediaType': oci.MANIFEST, 'digest': 'sha256:' + digest, 'size': size,
                'platform': {'os': 'linux', 'architecture': architecture}}]}))
    oci.image(destination, architecture)
    result = {'sourceSaveSha256': source_sha, 'manifestSha256': digest,
              'configSha256': config_sha, 'architecture': architecture,
              'layers': len(selected), 'packedBytes': packed, 'unpackedBytes': unpacked}
    with (destination / 'export-proof.json').open('x') as output:
        json.dump(result, output, indent=2)
        output.write('\n')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source-save', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--architecture', required=True, choices=('arm64', 'amd64'))
    args = parser.parse_args()
    print(json.dumps(export(args.source_save, args.output, args.architecture)))


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        code = str(error) if isinstance(error, ValueError) else ''
        print('P23_IMAGE_EXPORT_FAILED: ' + (code if re.fullmatch('[A-Z_]+', code) else type(error).__name__))
        raise SystemExit(1)
