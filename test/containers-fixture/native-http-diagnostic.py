#!/usr/bin/env python3
"""Compare pinned KJ HTTP persistence before and after its owning protocol fix."""

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import resource
import shutil
import signal
import stat
import subprocess
import sys
import tarfile
import threading
import time
import urllib.request

SPEC = importlib.util.spec_from_file_location(
    'diagnostic_inputs', Path(__file__).with_name('diagnostic-inputs.py'))
INPUTS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(INPUTS)


def dependency(source):
    """Accept only the immutable Cap'n Proto archive declared by the native fork."""
    block = re.search(r'http\.archive\(\s*name = "capnp-cpp",(.*?)\n\)',
                      source, re.DOTALL)
    if block is None:
        raise ValueError('CAPNP_DEPENDENCY_MISSING')
    values = {name: re.search(name + r' = "([^"]+)"', block[1])
              for name in ('sha256', 'url', 'strip_prefix')}
    if not all(values.values()):
        raise ValueError('CAPNP_DEPENDENCY_INCOMPLETE')
    result = {name: match[1] for name, match in values.items()}
    if (not re.fullmatch('[0-9a-f]{64}', result['sha256'])
            or not re.fullmatch(
                r'https://github\.com/capnproto/capnproto/tarball/[0-9a-f]{40}',
                result['url'])
            or not re.fullmatch(r'capnproto-capnproto-[0-9a-f]+/c\+\+',
                                result['strip_prefix'])):
        raise ValueError('CAPNP_DEPENDENCY_INVALID')
    return result


def phase(name, paths, commands, evidence):
    """Freeze each phase independently and terminate only its owned process group."""
    soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
    resource.setrlimit(resource.RLIMIT_NOFILE, (max(soft, len(paths) + 512), hard))
    held = INPUTS.HeldInputs(paths)
    stop = threading.Event()
    failures = []
    samples = []
    report = {'inputCount': len(held.entries), 'success': False}

    def observe():
        while not stop.wait(0.25):
            try:
                held.verify()
                current = shutil.disk_usage(evidence)
                samples.append((current.free, current.used / current.total))
                if current.used / current.total >= 0.95:
                    raise RuntimeError('DIAGNOSTIC_DISK_STOP_THRESHOLD')
            except Exception as error:
                failures.append(type(error).__name__)
                return

    observer = threading.Thread(target=observe, daemon=True)
    try:
        (evidence / (name + '-inputs.json')).write_text(json.dumps([
            {'path': str(path), 'stat': expected, 'sha256': digest}
            for path, _, expected, digest in held.entries], indent=2) + '\n')
        observer.start()
        for index, (command, expected) in enumerate(commands):
            if failures:
                raise RuntimeError('DIAGNOSTIC_OBSERVER_FAILED')
            with (evidence / f'{name}-{index}.log').open('xb') as output:
                process = subprocess.Popen(command, stdout=output,
                    stderr=subprocess.STDOUT, start_new_session=True)
                while process.poll() is None:
                    if failures:
                        os.killpg(process.pid, signal.SIGTERM)
                        try:
                            process.wait(timeout=10)
                        except subprocess.TimeoutExpired:
                            os.killpg(process.pid, signal.SIGKILL)
                            process.wait()
                    else:
                        time.sleep(0.25)
                if process.returncode != expected:
                    raise RuntimeError('DIAGNOSTIC_COMMAND_EXIT_MISMATCH')
        report['success'] = not failures
    finally:
        stop.set()
        if observer.ident is not None:
            observer.join()
        report.update(observerFailures=failures, observations=len(samples),
            minimumFreeBytes=min((item[0] for item in samples), default=0),
            maximumUsedFraction=max((item[1] for item in samples), default=0))
        try:
            held.verify(content=True)
            report['heldAndCurrentPathVerified'] = True
        finally:
            held.close()
            report['allDescriptorsClosed'] = True
            (evidence / (name + '-guard.json')).write_text(
                json.dumps(report, indent=2) + '\n')
    if failures:
        raise RuntimeError('DIAGNOSTIC_OBSERVER_FAILED')


def main():
    fork = Path(sys.argv[1]).resolve()
    root = Path.cwd()
    if not fork.is_relative_to(root / '.temp'):
        raise ValueError('DIAGNOSTIC_FORK_PATH_INVALID')
    if os.uname().machine != 'x86_64':
        raise RuntimeError('DIAGNOSTIC_REQUIRES_NATIVE_X64')
    disk = shutil.disk_usage(root)
    required = (44 << 30) + disk.total // 20 + 1
    if disk.free < required:
        raise RuntimeError('DIAGNOSTIC_DISK_ADMISSION_FAILED')
    work = root / '.temp/kj-http-diagnostic'
    work.mkdir(mode=0o700)
    evidence = work / 'evidence'
    evidence.mkdir()
    spec = dependency((fork / 'build/deps/gen/deps.MODULE.bazel').read_text())
    archive = work / 'source.tar.gz'
    with urllib.request.urlopen(spec['url'], timeout=60) as response, archive.open('xb') as output:
        shutil.copyfileobj(response, output)
    with archive.open('rb') as stream:
        if hashlib.file_digest(stream, 'sha256').hexdigest() != spec['sha256']:
            raise RuntimeError('CAPNP_ARCHIVE_DIGEST_MISMATCH')
    source = work / 'source'
    source.mkdir()
    prefix = spec['strip_prefix'] + '/'
    with tarfile.open(archive) as bundle:
        members = []
        for item in bundle.getmembers():
            if item.name.startswith(prefix):
                item.name = item.name[len(prefix):]
                if item.name:
                    members.append(item)
        bundle.extractall(source, members=members, filter='data')
    patches = fork / 'patches/capnp'
    production = patches / '0001-Honor-HTTP-1.0-default-connection-close.patch'
    regression = patches / '0002-Test-HTTP-1.0-connection-persistence.patch'
    subprocess.run(['patch', '-p1', '-d', str(source), '-i', str(regression)], check=True)
    probe = work / 'probe'
    probe.mkdir()
    (probe / 'CMakeLists.txt').write_text(
        'cmake_minimum_required(VERSION 3.20)\n'
        'project(kj_http_diagnostic LANGUAGES CXX)\n'
        'set(CMAKE_CXX_STANDARD 23)\n'
        'add_subdirectory("../source" library)\n'
        'add_executable(http-policy-test ../source/src/kj/compat/http-test.c++)\n'
        'target_link_libraries(http-policy-test PRIVATE CapnProto::kj-http CapnProto::kj-test)\n')
    build = work / 'build'
    configure = ['cmake', '-S', str(probe), '-B', str(build),
        '-DBUILD_TESTING=OFF', '-DBUILD_SHARED_LIBS=OFF', '-DWITH_OPENSSL=OFF',
        '-DWITH_ZLIB=OFF', '-DCMAKE_BUILD_TYPE=Debug', '-DCMAKE_CXX_FLAGS_DEBUG=-O0 -g0']
    compile_test = ['cmake', '--build', str(build), '--target', 'http-policy-test', '--parallel', '2']
    execute = [str(build / 'http-policy-test')]

    def frozen_paths():
        return [path for directory in (source, probe, patches)
                for path in directory.rglob('*') if stat.S_ISREG(path.lstat().st_mode)] + [
                    archive, Path(__file__).resolve(),
                    Path(__file__).with_name('diagnostic-inputs.py').resolve(),
                    fork / 'build/deps/gen/deps.MODULE.bazel']

    phase('before', frozen_paths(), [(configure, 0), (compile_test, 0), (execute, 1)], evidence)
    if 'HttpHeaders HTTP/1.0 default connection persistence' not in (
            evidence / 'before-2.log').read_text():
        raise RuntimeError('DIAGNOSTIC_REGRESSION_NOT_OBSERVED')
    subprocess.run(['patch', '-p1', '-d', str(source), '-i', str(production)], check=True)
    phase('fixed', frozen_paths(), [(compile_test, 0), (execute, 0)], evidence)
    (evidence / 'result.json').write_text(json.dumps({
        'success': True, 'diagnosticOnly': True, 'formalRuntimePinUnchanged': True,
        'dependency': spec, 'originalRequiredFreeBytes': required,
        'initialFreeBytes': disk.free, 'beforeExitCode': 1, 'fixedExitCode': 0,
        'rustOrProductAcceptanceClaimed': False}, indent=2) + '\n')
    print('Pinned KJ owning HTTP regression failed before the fix and passed after it.', flush=True)


if __name__ == '__main__':
    main()
