#!/usr/bin/env python3
"""Build native P23 fixture images and export verified OCI artifacts."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import uuid

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
spec = importlib.util.spec_from_file_location('export_image', HERE / 'export-image.py')
exporter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(exporter)
oci = exporter.oci
ALPINE = 'alpine@sha256:ce64758a109eb420d874a118f87920e625e12d3634e03b4a5573fd9f6e5d3507'
IMAGES = ('sidecar', 'one', 'two', 'qualification')
SOURCE_FILES = ('test/containers-fixture/Dockerfile.base',
                'test/containers-fixture/Dockerfile.sidecar',
                'test/applications/containers/Dockerfile',
                'test/applications/containers/serve.sh')


def copy_elf(source, destination, architecture):
    oci.require(source.is_absolute() and source.resolve() == source, 'BUILD_BINARY_PATH')
    descriptor = os.open(source, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    with os.fdopen(descriptor, 'rb') as stream:
        before = os.fstat(stream.fileno())
        oci.require(stat.S_ISREG(before.st_mode) and before.st_mode & 0o111
                    and 64 <= before.st_size <= 128 << 20, 'BUILD_BINARY_TYPE')
        header = stream.read(20)
        oci.require(header[:6] == b'\x7fELF\x02\x01'
                    and int.from_bytes(header[18:20], 'little') == {
                        'arm64': 183, 'amd64': 62}[architecture], 'BUILD_BINARY_PLATFORM')
        stream.seek(0)
        with destination.open('xb') as output:
            shutil.copyfileobj(stream, output, 1 << 20)
        digest = oci.regular(destination, 128 << 20)[0]
        stream.seek(0)
        oci.require(hashlib.file_digest(stream, 'sha256').hexdigest() == digest
                    and oci.identity(before) == oci.identity(os.fstat(stream.fileno()))
                    and oci.identity(before) == oci.identity(source.lstat()), 'BUILD_BINARY_CHANGED')
    destination.chmod(0o755)
    return digest


def build(docker_host, mbx, sidecar, output, selected=None):
    oci.require(selected is None or selected in IMAGES, 'BUILD_SELECTED_IMAGE')
    architecture = {'aarch64': 'arm64', 'arm64': 'arm64', 'x86_64': 'amd64'}.get(platform.machine())
    oci.require(platform.system() == 'Linux' and architecture is not None, 'BUILD_NATIVE_LINUX_REQUIRED')
    oci.require(docker_host.startswith('unix:///') and '\n' not in docker_host,
                'BUILD_EXPLICIT_DOCKER_SOCKET')
    socket = Path(docker_host[len('unix://'):])
    oci.require(socket.resolve() == socket and stat.S_ISSOCK(socket.lstat().st_mode),
                'BUILD_DOCKER_SOCKET')
    socket_before = oci.identity(socket.lstat())
    oci.require(output.is_absolute() and output.parent.resolve() == output.parent
                and not os.path.lexists(output), 'BUILD_FRESH_OUTPUT')
    docker = shutil.which('docker')
    oci.require(docker is not None, 'BUILD_DOCKER_MISSING')
    output.mkdir(mode=0o700)
    results = {}
    image_ids = {}
    docker_archives = {}
    tags = []
    with tempfile.TemporaryDirectory(prefix='build-', dir=output) as temporary:
        work = Path(temporary)
        config = work / 'docker-config'
        config.mkdir(mode=0o700)
        env = {'PATH': os.environ.get('PATH', os.defpath), 'HOME': str(work),
               'DOCKER_HOST': docker_host, 'DOCKER_CONFIG': str(config)}

        def run(*args, capture=False):
            return subprocess.run([docker, *args], env=env, check=True, text=True,
                                  stdout=subprocess.PIPE if capture else None)

        info = json.loads(run('info', '--format', '{{json .}}', capture=True).stdout)
        oci.require(info.get('OSType') == 'linux' and info.get('Architecture') in {
            'arm64': ('aarch64', 'arm64'), 'amd64': ('x86_64', 'amd64')}[architecture],
            'BUILD_ENGINE_PLATFORM')
        contexts = {}
        for name in ('base', 'sidecar', 'application'):
            contexts[name] = work / name
            contexts[name].mkdir(mode=0o700)
        binary_inputs = {
            'mbxSha256': copy_elf(mbx, contexts['base'] / 'mbx', architecture),
            'sidecarSha256': copy_elf(sidecar, contexts['sidecar'] / 'container-sidecar', architecture)}
        version = subprocess.run([str(contexts['base'] / 'mbx'), '--version'], check=True, text=True,
                                 stdout=subprocess.PIPE, env=env).stdout.strip()
        oci.require(version == 'mbx 1.22.0', 'BUILD_MBX_VERSION')
        sources = {HERE / ('Dockerfile.' + name): contexts[name] / 'Dockerfile'
                   for name in ('base', 'sidecar')}
        sources.update({ROOT / 'test/applications/containers' / name: contexts['application'] / name
                        for name in ('Dockerfile', 'serve.sh')})
        oci.require({str(path.relative_to(ROOT)) for path in sources} == set(SOURCE_FILES),
                    'BUILD_SOURCE_FIELDS')
        source_inputs = {}
        for source, destination in sources.items():
            digest, size, raw = oci.regular(source, 1 << 20, True)
            source_inputs[str(source.relative_to(ROOT))] = {'sha256': digest, 'bytes': size}
            with destination.open('xb') as stream:
                stream.write(raw)
        prefix = 'open-compute-fixture-' + uuid.uuid4().hex
        try:
            names = ['base'] if selected in (None, 'one', 'two') else []
            names += [name for name in ('sidecar', 'one', 'two') if selected in (None, name)]
            for name in names:
                tag = prefix + ':' + name
                args = ['build', '--platform', 'linux/' + architecture, '--tag', tag]
                context = contexts[name] if name in contexts else contexts['application']
                if name in ('one', 'two'):
                    args += ['--build-arg', 'BASE_IMAGE=' + prefix + ':base',
                             '--build-arg', 'REVISION=' + name]
                run(*args, str(context))
                tags.append(tag)
                image_id = run('image', 'inspect', '--format', '{{.Id}}', tag, capture=True).stdout.strip()
                oci.require(re.fullmatch('sha256:[0-9a-f]{64}', image_id) is not None,
                            'BUILD_IMAGE_ID')
                image_ids[name] = image_id
                if name != 'base':
                    archive = work / (name + '.tar')
                    run('image', 'save', '--output', str(archive), image_id)
                    results[name] = exporter.export(archive, output / name, architecture, image_id)
                    # Artifact identities use verified config digests across Engine image stores.
                    image_ids[name] = 'sha256:' + results[name]['configSha256']
                    if name == 'sidecar':
                        target = output / 'sidecar.docker.tar'
                        archive.rename(target)
                        docker_archives[name] = {'path': target.name,
                            'sha256': results[name]['sourceSaveSha256'], 'bytes': target.stat().st_size}
                    else:
                        archive.unlink()
            # Qualification must not contain the sidecar helper, which archives exclude.
            if selected in (None, 'qualification'):
                run('pull', '--platform', 'linux/' + architecture, ALPINE)
                archive = work / 'qualification.tar'
                run('image', 'save', '--output', str(archive), ALPINE)
                results['qualification'] = exporter.export(archive, output / 'qualification', architecture)
                target = output / 'qualification.docker.tar'
                archive.rename(target)
                docker_archives['qualification'] = {'path': target.name,
                    'sha256': results['qualification']['sourceSaveSha256'], 'bytes': target.stat().st_size}
            if 'one' in results and 'two' in results:
                oci.require(results['one']['manifestSha256'] != results['two']['manifestSha256'],
                            'BUILD_DISTINCT_IMAGES')
            for source in sources:
                digest, size, _ = oci.regular(source, 1 << 20)
                oci.require(source_inputs[str(source.relative_to(ROOT))] == {
                    'sha256': digest, 'bytes': size}, 'BUILD_SOURCE_CHANGED')
            oci.require(oci.identity(socket.lstat()) == socket_before, 'BUILD_ENGINE_SOCKET_CHANGED')
        finally:
            # Remove only this invocation's tags; shared image layers remain Engine-owned cache.
            cleanup_errors = []
            for tag in reversed(tags):
                try:
                    run('image', 'rm', tag)
                except subprocess.CalledProcessError as error:
                    cleanup_errors.append(error)
            oci.require(not cleanup_errors, 'BUILD_TAG_CLEANUP_FAILED')
    result = {'schemaVersion': 1, 'architecture': architecture, 'selectedImage': selected,
              'binaryInputs': binary_inputs,
              'sourceInputs': source_inputs, 'engineImageIds': image_ids, 'images': results,
              'dockerArchives': docker_archives}
    (output / 'images.json').write_text(json.dumps(result, indent=2) + '\n')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--docker-host', required=True)
    parser.add_argument('--mbx', required=True, type=Path)
    parser.add_argument('--sidecar', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--image', choices=IMAGES, help='Retry only one failed artifact; default builds all images')
    args = parser.parse_args()
    print(json.dumps(build(args.docker_host, args.mbx, args.sidecar, args.output, args.image)))


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        code = str(error) if isinstance(error, ValueError) else ''
        if re.fullmatch('[A-Z_]+', code) is None:
            code = type(error).__name__
        print('P23_IMAGE_BUILD_FAILED: ' + code, file=sys.stderr)
        raise SystemExit(1)
