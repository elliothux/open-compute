#!/usr/bin/env python3
"""Collect evidence for the rejected release candidate, without qualifying a release."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import threading
import time

CASES = {
    'r2': ('p0_5_r2_gate', ('uploads::concurrent_large_upload_keeps_runtime_responsive',)),
    'lifecycle': ('single_binary', (
        'one_daemon_starts_two_isolated_instance_children',
        'single_file_first_start_restart_orphan_recovery_and_corruption_failure',
    )),
}


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def owned_processes(text, root_pid, uid):
    rows = {}
    for line in text.splitlines():
        fields = line.strip().split(None, 7)
        if len(fields) != 8:
            continue
        try:
            pid, parent, owner = map(int, fields[:3])
        except ValueError:
            continue
        rows[pid] = {'pid': pid, 'parent': parent, 'uid': owner,
                     'elapsed': fields[3], 'cpuPercent': fields[4],
                     'rssKiB': fields[5], 'state': fields[6], 'image': fields[7]}
    if root_pid not in rows or rows[root_pid]['uid'] != uid:
        return []
    result = []
    for pid, row in rows.items():
        cursor, seen = pid, set()
        while cursor in rows and cursor != root_pid and cursor not in seen:
            seen.add(cursor)
            cursor = rows[cursor]['parent']
        if row['uid'] == uid and cursor == root_pid:
            result.append(row)
    return result


def observe(child, out, mode, stop):
    sampled, first_seen, next_memory = set(), {}, 0.0
    with (out / 'processes.jsonl').open('x') as log:
        while not stop.wait(0.5) and child.poll() is None:
            now = time.monotonic()
            snapshot = subprocess.run(
                ['ps', '-axo', 'pid=,ppid=,uid=,etime=,pcpu=,rss=,stat=,comm='],
                capture_output=True, text=True, check=False)
            rows = owned_processes(snapshot.stdout, child.pid, os.getuid())
            log.write(json.dumps({'unixNs': time.time_ns(), 'loadAverage': os.getloadavg(),
                                  'processes': rows}) + '\n')
            if now >= next_memory:
                memory = subprocess.run(['vm_stat'], capture_output=True, check=False)
                with (out / 'memory.log').open('ab') as metrics:
                    metrics.write(f'unixNs={time.time_ns()} exitCode={memory.returncode}\n'.encode())
                    metrics.write(memory.stdout)
                next_memory = now + 5
            log.flush()
            active_upload = mode != 'r2'
            if mode == 'r2' and len(sampled) < 8:
                files = subprocess.run(
                    ['/usr/sbin/lsof', '-a', '-p', str(child.pid), '-Fn'],
                    capture_output=True, text=True, check=False)
                active_upload = any(line.startswith('n') and '/r2-staging/' in line
                                    for line in files.stdout.splitlines())
            for row in rows:
                pid = row['pid']
                first_seen.setdefault(pid, now)
                image = Path(row['image']).name
                if (pid in sampled or len(sampled) >= 8 or not active_upload
                        or (pid != child.pid and image not in {'ocd', 'workerd'})
                        or (mode == 'lifecycle' and now - first_seen[pid] < 5)):
                    continue
                # Re-check the descendant relation before reading this process's stacks.
                current = subprocess.run(
                    ['ps', '-axo', 'pid=,ppid=,uid=,etime=,pcpu=,rss=,stat=,comm='],
                    capture_output=True, text=True, check=False)
                if not any(p['pid'] == pid and p['image'] == row['image']
                           for p in owned_processes(current.stdout, child.pid, os.getuid())):
                    continue
                sampled.add(pid)
                result = subprocess.run(
                    ['/usr/bin/sample', str(pid), '1', '10', '-file',
                     str(out / f'owned-{pid}.sample')], capture_output=True, check=False)
                log.write(json.dumps({'unixNs': time.time_ns(), 'sampledPid': pid,
                                      'sampleExitCode': result.returncode}) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mode', choices=CASES, required=True)
    args = parser.parse_args()
    root = Path.cwd()
    expected = os.environ['DIAGNOSTIC_SOURCE_REVISION']
    assert re.fullmatch('[0-9a-f]{40}', expected)
    assert subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip() == expected
    subprocess.run(['git', 'diff', '--exit-code'], check=True)
    assert os.uname().sysname == 'Darwin' and os.uname().machine == 'x86_64'
    workerd = Path(os.environ['OPEN_COMPUTE_TEST_WORKERD'])
    lock = root / 'packages/runtime/workerd.lock.json'
    pin = json.loads(lock.read_text())
    assert workerd.is_absolute() and sha(workerd) == pin['targets']['darwin-x64']['binarySha256']
    for key in ('build_script_execution', 'target.views', 'restore_hardlink', 'gc.auto'):
        assert subprocess.check_output(['mbx', 'settings', 'get', key], text=True).strip() == 'false'
    assert shutil.disk_usage(root).free >= 44 << 30
    target, cases = CASES[args.mode]
    out = root / '.temp' / f'intel-diagnostics-{args.mode}'
    out.mkdir(mode=0o700)
    report = {'diagnosticOnly': True, 'releaseQualified': False, 'revision': expected,
              'pinSha256': sha(lock), 'workerdSha256': sha(workerd), 'mode': args.mode,
              'samplingMayPerturbTiming': True, 'cases': []}
    for operation, command in [('hardware', ['sysctl', 'hw.memsize', 'hw.ncpu']),
                               ('memory-before', ['vm_stat'])]:
        result = subprocess.run(command, capture_output=True, check=False)
        (out / f'{operation}.log').write_bytes(result.stdout)
    begin = time.monotonic()
    with (out / 'compile.jsonl').open('x') as stdout, (out / 'compile.stderr.log').open('x') as stderr:
        build = subprocess.run(['mbx', 'test', '--locked', '--offline', '--all-features',
                                '-p', 'open-compute-service', '--test', target, '--no-run',
                                '--message-format=json'], stdout=stdout, stderr=stderr, check=False)
    report['compile'] = {'exitCode': build.returncode, 'seconds': time.monotonic() - begin}
    if build.returncode == 0:
        binaries = set()
        for line in (out / 'compile.jsonl').read_text().splitlines():
            try:
                item = json.loads(line)
            except ValueError:
                continue
            if item.get('executable') and item.get('target', {}).get('name') == target:
                binaries.add(item['executable'])
        assert len(binaries) == 1
        binary = binaries.pop()
        listing = subprocess.run([binary, '--list'], capture_output=True, text=True, check=True)
        (out / 'list.log').write_text(listing.stdout + listing.stderr)
        discovered = {line.removesuffix(': test') for line in listing.stdout.splitlines()
                      if line.endswith(': test')}
        assert set(cases) <= discovered
        report['discovered'] = len(discovered)
        report['binarySha256'] = sha(Path(binary))
        for index, case in enumerate(cases):
            assert shutil.disk_usage(root).free >= 44 << 30
            folder = out / f'case-{index}'
            folder.mkdir()
            stop = threading.Event()
            begin = time.monotonic()
            with (folder / 'test.log').open('x') as log:
                child = subprocess.Popen([binary, case, '--exact', '--nocapture', '--test-threads=1'],
                                         stdout=log, stderr=subprocess.STDOUT)
                observation_errors = []
                def collect():
                    try:
                        observe(child, folder, args.mode, stop)
                    except Exception as error:
                        observation_errors.append(type(error).__name__)
                observer = threading.Thread(target=collect)
                observer.start()
                code = child.wait()
                stop.set()
                observer.join()
            report['cases'].append({'case': case, 'exitCode': code,
                                    'seconds': time.monotonic() - begin,
                                    'testLogSha256': sha(folder / 'test.log'),
                                    'observationErrors': observation_errors})
            (out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    subprocess.run(['git', 'diff', '--exit-code'], check=True)
    assert sha(lock) == report['pinSha256'] and sha(workerd) == report['workerdSha256']
    report['sourceUnchanged'] = True
    report['collectionCompleted'] = True
    (out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    return 0 if build.returncode == 0 and all(case['exitCode'] == 0 and not case['observationErrors'] for case in report['cases']) else 1


if __name__ == '__main__':
    raise SystemExit(main())
