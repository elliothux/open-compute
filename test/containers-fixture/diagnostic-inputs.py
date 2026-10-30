#!/usr/bin/env python3
"""Freeze disposable CI diagnostic inputs while the canonical Container Gate runs."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import resource
import shutil
import signal
import stat
import subprocess
import threading
import time


def identity(value):
    """Include file identity, ownership, links, permissions, size and both timestamps."""
    return tuple(getattr(value, field) for field in (
        'st_dev', 'st_ino', 'st_mode', 'st_nlink', 'st_uid', 'st_gid',
        'st_size', 'st_mtime_ns', 'st_ctime_ns'))


def sha256(fd):
    """Hash the held descriptor without trusting a replacement pathname."""
    os.lseek(fd, 0, os.SEEK_SET)
    result = hashlib.sha256()
    while block := os.read(fd, 1 << 20):
        result.update(block)
    return result.hexdigest()


class HeldInputs:
    """Own read-only descriptors and compare each descriptor with its current path."""

    def __init__(self, paths):
        self.entries = []
        try:
            for path in sorted(set(paths)):
                fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
                self.entries.append((path, fd, identity(os.fstat(fd)), sha256(fd)))
            self.verify(content=True)
        except BaseException:
            self.close()
            raise

    def verify(self, content=False):
        """Reject path replacement, metadata drift and final byte drift."""
        for path, fd, expected, digest in self.entries:
            if identity(os.fstat(fd)) != expected or identity(path.lstat()) != expected:
                raise RuntimeError('DIAGNOSTIC_INPUT_IDENTITY_CHANGED')
            if content:
                current = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
                try:
                    if (identity(os.fstat(current)) != expected or sha256(fd) != digest
                            or sha256(current) != digest):
                        raise RuntimeError('DIAGNOSTIC_INPUT_BYTES_CHANGED')
                finally:
                    os.close(current)

    def close(self):
        """Close every descriptor, including after incomplete admission."""
        for _, fd, _, _ in self.entries:
            os.close(fd)
        self.entries.clear()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    args = parser.parse_args()
    root = Path.cwd()
    evidence = root / '.temp/container-diagnostic'
    os.environ['LLVM_PROFILE_FILE'] = str(evidence / 'profile-%12m.profraw')
    paths = [root / os.fsdecode(name) for name in subprocess.check_output(
        ['git', 'ls-files', '-z']).split(b'\0') if name]
    for directory in ('packages/runtime/dist', 'packages/sdk/dist',
                      'test/applications/containers/.cloudflare'):
        paths.extend((root / directory).rglob('*'))
    paths += [args.binary, Path(os.environ['OPEN_COMPUTE_TEST_WORKERD']),
              Path(os.environ['OPEN_COMPUTE_BUILD_CADDY']),
              Path(os.environ['OPEN_COMPUTE_TEST_CONTAINERS_INPUT'])]
    paths = [path for path in paths if stat.S_ISREG(path.lstat().st_mode)]
    soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
    resource.setrlimit(resource.RLIMIT_NOFILE, (max(soft, len(paths) + 512), hard))
    disk = shutil.disk_usage(root)
    required = (44 << 30) + disk.total // 20 + 1
    if disk.free < required:
        raise RuntimeError('DIAGNOSTIC_DISK_ADMISSION_FAILED')
    held = HeldInputs(paths)
    (evidence / 'inputs.json').write_text(json.dumps([
        {'path': str(path), 'stat': expected, 'sha256': digest}
        for path, _, expected, digest in held.entries], indent=2) + '\n')
    stop = threading.Event()
    failures = []
    samples = []
    result = {'inputCount': len(held.entries), 'requiredFreeBytes': required,
              'initialFreeBytes': disk.free, 'success': False}
    process = None

    def observe():
        while not stop.wait(0.25):
            try:
                held.verify()
                current = shutil.disk_usage(root)
                samples.append((current.free, current.used / current.total))
                if current.used / current.total >= 0.95:
                    raise RuntimeError('DIAGNOSTIC_DISK_STOP_THRESHOLD')
            except Exception as error:
                failures.append(type(error).__name__)
                if process is not None and process.poll() is None:
                    os.killpg(process.pid, signal.SIGTERM)
                return

    observer = threading.Thread(target=observe, daemon=True)
    try:
        observer.start()
        for options in (['--list'], ['--keep-going', '--jobs', '1']):
            process = subprocess.Popen(['./test/gate.py', 'p23-containers', *options],
                                       start_new_session=True)
            while process.poll() is None:
                if failures:
                    os.killpg(process.pid, signal.SIGTERM)
                    try:
                        process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                else:
                    time.sleep(0.25)
            result['commandExitCode'] = process.returncode
            if process.returncode or failures:
                break
        held.verify(content=True)
        result['success'] = result['commandExitCode'] == 0 and not failures
    finally:
        stop.set()
        observer.join()
        result.update({'observerFailures': failures, 'observations': len(samples),
                       'minimumFreeBytes': min((item[0] for item in samples), default=disk.free),
                       'maximumUsedFraction': max((item[1] for item in samples), default=0),
                       'heldAndCurrentPathVerified': False})
        try:
            held.verify(content=True)
            result['heldAndCurrentPathVerified'] = True
        finally:
            held.close()
            result['allDescriptorsClosed'] = True
            (evidence / 'input-guard.json').write_text(json.dumps(result, indent=2) + '\n')
    raise SystemExit(0 if result['success'] else 1)


if __name__ == '__main__':
    main()
