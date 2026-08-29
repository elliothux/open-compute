#!/usr/bin/env python3
"""Build once; run every selected case once, then repeat only audited timing cases."""

import argparse
from collections import namedtuple
import concurrent.futures
from functools import partial
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import resource
import subprocess
import sys
import tempfile
import time
import uuid

# This CLI imports repository-local policy; do not create disposable source-tree
# bytecode (or change the frozen input set) while discovering/running tests.
sys.dont_write_bytecode = True
from gate_cases import ONCE, TIMING, validate_registry

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_JOBS = min(4, os.cpu_count() or 1)
Target = namedtuple('Target', 'package_id name kind cwd exclusive cases', defaults=[None])
# Only these targets have been audited for independent TempDir/SQLite/S3/port-0 state.
# p0_1 scans global executable staging, so it is an exclusive barrier.
# The current Workflow product target's 16 concurrent MiB results exhausted
# shared kernel socket buffers in a
# parallel measurement. Keep the workload intact and isolate its test process.
TARGETS = {
    'p0-1': ('open-compute-service', 'p0_1_gate', True),
    'p0-2': ('open-compute-service', 'p0_2_runtime_gate', False),
    'p0-3': ('open-compute-service', 'p0_3_resource_binding_gate', False),
    'p0-4': ('open-compute-service', 'p0_4_kv_gate', False),
    'p0-5': ('open-compute-service', 'p0_5_r2_gate', False),
    'p0-6': ('open-compute-service', 'p0_6_d1_gate', False),
    'p0-7': ('open-compute-service', 'p0_7_durable_objects_gate', False),
    'p0-8': ('open-compute-service', 'p0_8_scheduler_do_alarms_gate', False),
    'p0-exit': ('open-compute-service', 'p0_exit_gate', False),
    'p1-conformance': ('open-compute-service', 'p1_conformance', False),
    'p1-security': ('open-compute-service', 'p1_security', False),
    'p1-crash': ('open-compute-service', 'p1_crash_process', False),
    'p1-snapshot': ('open-compute-service', 'p1_snapshot_restore', False),
    'p2-1': ('open-compute-service', 'p2_1_scheduler_hardening_gate', False),
    'p2-2': ('open-compute-service', 'p2_2_queue_producer_gate', False),
    'workflow-runtime': ('open-compute-service', 'workflow_runtime_gate', False),
    'workflow-recovery': ('open-compute-service', 'workflow_recovery_gate', False),
    'p2-exit': ('open-compute-service', 'p2_exit_gate', False),
    # Finish independent work together before the remaining exclusive barriers.
    'workflow-product': ('open-compute-service', 'workflow_product_gate', True),
    'runtime': ('open-compute-runtime', 'supervisor', True),
    'single-binary': ('open-compute-service', 'single_binary', True),
}
GROUPS = {
    'p0': [name for name in TARGETS if name.startswith('p0-')],
    'p1': [name for name in TARGETS if name.startswith('p1-')],
    'p2': [name for name in TARGETS if name.startswith('p2-') or name.startswith('workflow-')],
    # Queue/Cron share the immutable loader matrix; selecting both never duplicates it.
    'p2-3': ['p0-2'],
    'p1-8': ['p0-7', 'p1-conformance'],
    'workflow': ['workflow-runtime', 'workflow-recovery', 'workflow-product'],
    'all': list(TARGETS),
}


def selection(names):
    selected = set()
    for name in names:
        if name not in TARGETS and name not in GROUPS:
            raise ValueError(f'unknown Gate target: {name}')
        selected.update(GROUPS.get(name, [name]))
    return [name for name in TARGETS if name in selected]


def rounds_from_env():
    value = os.environ.get('OPEN_COMPUTE_GATE_ROUNDS', '1')
    if value not in ('1', '3'):
        raise ValueError('OPEN_COMPUTE_GATE_ROUNDS must be 1 or 3')
    return int(value)


def round_plan(targets, rounds):
    """A full first pass owns coverage; later fresh processes own timing samples."""
    result = [targets]
    timing = {name: target._replace(cases=tuple(sorted(TIMING[name])))
              for name, target in targets.items() if TIMING.get(name)}
    if rounds == 3 and timing:
        result.extend([timing, timing])
    return result


def plan_summary(plans):
    return [{'round': number, 'targets': {name: target.cases for name, target in targets.items()}}
            for number, targets in enumerate(plans, 1)]


def discovered_cases(log):
    """Parse libtest discovery, including empty workspace binary harnesses."""
    raw = Path(log).read_text()
    cases = re.findall(r'^([^\s]+): test$', raw, re.MULTILINE)
    summary = re.findall(r'^(\d+) tests?, (\d+) benchmarks?$', raw, re.MULTILINE)
    if (len(summary) != 1 or tuple(map(int, summary[0])) != (len(cases), 0)
            or len(cases) != len(set(cases))):
        raise ValueError(f'invalid or unsupported libtest inventory: {log}')
    return tuple(sorted(cases))


def verify_case_inventory(targets, prepared):
    """Do not turn a renamed, new, or filtered-out test into successful acceptance."""
    inventories = {item['target']: discovered_cases(item['log']) for item in prepared}
    for name in targets.keys() & TARGETS.keys():
        expected = set(ONCE.get(name, ())) | set(TIMING.get(name, ()))
        actual = set(inventories[name])
        if expected != actual:
            raise ValueError(f'{name}: case registry mismatch; missing={sorted(expected - actual)}; '
                             f'unregistered={sorted(actual - expected)}')
    return {name: target._replace(cases=inventories[name]) for name, target in targets.items()}


def resolve_targets(selected, workspace):
    """Use Cargo's workspace inventory; unaudited targets remain exclusive."""
    metadata = json.loads(subprocess.check_output(
        [os.environ.get('CARGO', 'cargo'), 'metadata', '--locked', '--offline',
         '--no-deps', '--format-version=1'], cwd=ROOT, text=True))
    known = {(package, name): (label, exclusive)
             for label, (package, name, exclusive) in TARGETS.items()}
    independent = {
        ('open-compute-core', 'lib'), ('open-compute-storage', 'lib'),
        ('open-compute-artifacts', 'lib'), ('open-compute-workers', 'lib'),
        ('open-compute-service', 'lib'),
        ('open-compute-service', 'msrv_audit'),
        ('open-compute-service', 'p1_reliability'),
    }
    found = {}
    for package in metadata['packages']:
        if package['id'] not in metadata['workspace_members']:
            continue
        for target in package['targets']:
            if not target['test'] or target['kind'] == ['custom-build']:
                continue
            kind = target['kind'][0]
            label, exclusive = known.get((package['name'], target['name']), (
                f'{package["name"]}.{kind}.{target["name"]}',
                (package['name'], kind if kind == 'lib' else target['name']) not in independent))
            if workspace or label in selected:
                found[label] = Target(package['id'], target['name'], kind,
                                      str(Path(package['manifest_path']).parent), exclusive,
                                      tuple(sorted(ONCE.get(label, ()) + TIMING.get(label, ())))
                                      if label in TARGETS else None)
    if workspace:
        missing = TARGETS.keys() - found.keys()
        if missing:
            raise ValueError(f'Cargo workspace is missing registered Gate targets: {sorted(missing)}')
        # CLI first loads the actual platformd executable before timed runtime probes.
        # Runtime's tight process-fault windows require an exclusive slot even though
        # its hooks/staging are private; the other libraries have passed together.
        # Complete independent jobs together; unaudited/global-state jobs are barriers.
        return dict(sorted(found.items(), key=lambda item: (
            0 if item[0] == 'open-compute-service.test.cli' else 2 if item[1].exclusive else 1,
            item[0])))
    missing = set(selected) - found.keys()
    if missing:
        raise ValueError(f'Cargo metadata is missing selected targets: {sorted(missing)}')
    return {name: found[name] for name in selected}


def digest(path):
    with path.open('rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()


def source_identity():
    names = subprocess.check_output(
        ['git', 'ls-files', '-z', '--cached', '--others', '--exclude-standard'], cwd=ROOT,
    ).split(b'\0')
    result = hashlib.sha256()
    for name in sorted(set(names) - {b''}):
        # Designs/history are not runtime inputs. Maintained references include
        # embedded runbooks and conformance fixtures, so they must remain frozen.
        if name.startswith(b'docs/') and not name.startswith(b'docs/references/'):
            continue
        path = ROOT / os.fsdecode(name)
        result.update(name + b'\0')
        result.update(digest(path).encode() if path.is_file() else b'deleted')
    return result.hexdigest()


def verify_inputs(*, probe_version=True):
    lock_path = ROOT / 'packages/runtime/workerd.lock.json'
    pin = json.loads(lock_path.read_text())
    arch = {'aarch64': 'arm64', 'arm64': 'arm64', 'x86_64': 'x64', 'AMD64': 'x64'}.get(platform.machine())
    target = f'{sys.platform}-{arch}'
    entry = pin['targets'].get(target)
    if entry is None:
        raise ValueError(f'formal pin does not support {target}')
    result = {'target': target, 'pin_sha256': digest(lock_path), 'workerd': pin['release']}
    for variable, field in [('OPEN_COMPUTE_TEST_WORKERD', 'binarySha256'),
                            ('OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE', 'archiveSha256')]:
        path = Path(os.environ.get(variable, ''))
        if not path.is_absolute() or path.is_symlink() or not path.is_file():
            raise ValueError(f'{variable} must name an existing absolute regular file; no downloads')
        if digest(path) != entry[field]:
            raise ValueError(f'{variable} SHA-256 does not match the formal pin')
        result[field] = entry[field]
    if probe_version:
        version = subprocess.run([os.environ['OPEN_COMPUTE_TEST_WORKERD'], '--version'],
                                 capture_output=True, text=True, check=True, timeout=20)
        if version.stdout.strip() != pin['expectedVersionOutput']:
            raise ValueError('workerd version does not match the formal pin')
    manifest = ROOT / 'packages/runtime/dist/manifest.json'
    if not manifest.is_file():
        raise ValueError('runtime assets missing; run bun run build explicitly before the Gate')
    result['manifest_sha256'] = digest(manifest)
    return result


def build_targets(targets, directory, workspace):
    command = [os.environ.get('CARGO', 'cargo'), 'test', '--locked', '--offline',
               '--all-features', '--no-run', '--message-format=json']
    if workspace:
        command += ['--workspace', '--all-targets']
    else:
        for package in dict.fromkeys(target.package_id for target in targets.values()):
            command += ['-p', package]
        for target in dict.fromkeys(target.name for target in targets.values()):
            command += ['--test', target]
    start = time.monotonic()
    with (directory / 'build.stderr.log').open('x') as errors:
        result = subprocess.run(command, cwd=ROOT, stdout=subprocess.PIPE, stderr=errors, text=True)
    artifacts = {}
    expected = {(target.package_id, target.name, target.kind): name for name, target in targets.items()}
    with (directory / 'build.jsonl').open('x') as output:
        output.write(result.stdout)
    if result.returncode:
        raise RuntimeError(f'build failed; see {directory / "build.stderr.log"}')
    for line in result.stdout.splitlines():
        item = json.loads(line)
        if item.get('reason') == 'compiler-artifact' and item.get('executable') and item['profile']['test']:
            key = (item['package_id'], item['target']['name'], item['target']['kind'][0])
            if key not in expected:
                raise RuntimeError(f'Cargo produced an unplanned test executable: {key}')
            artifacts[expected[key]] = item['executable']
    missing = targets.keys() - artifacts.keys()
    if missing:
        raise RuntimeError(f'Cargo did not produce selected Gate executables: {missing}')
    return artifacts, {'invocations': 1, 'seconds': time.monotonic() - start,
                       'executables': {name: digest(Path(artifacts[name])) for name in targets}}


def execute_target(name, executable, directory, target, *, list_only=False):
    directory.mkdir()
    # Supervisor fixtures inherit a cleared environment from the production spawn
    # path. Keep their relative diagnostic output (including LLVM profiles) in this
    # run, without passing test variables through that security boundary.
    cwd = str(directory.resolve()) if name == 'runtime' else target.cwd
    # Keep pathname-based Unix sockets below SUN_LEN even with long target labels.
    # Retained evidence is moved beside the log only after the process has exited.
    temporary_root = ROOT / '.temp/gate-tmp'
    temporary_root.mkdir(parents=True, exist_ok=True)
    temporary = Path(tempfile.mkdtemp(prefix='', dir=temporary_root)).resolve()
    env = dict(os.environ, TMPDIR=str(temporary), TMP=str(temporary), TEMP=str(temporary))
    # Repetition belongs only to this runner, including when a test spawns its own fixture.
    env.pop('OPEN_COMPUTE_GATE_ROUNDS', None)
    start = time.monotonic()
    try:
        with (directory / 'output.log').open('x') as output:
            # Native harness loading can trigger slow host executable assessment. Finish
            # discovery before any product timeout starts; never prewarm product state.
            arguments = ['--list'] if list_only else ['--test-threads=1', '--nocapture']
            if not list_only and target.cases:
                arguments += ['--exact', *target.cases]
            process = subprocess.run([executable, *arguments], cwd=cwd, env=env,
                                     stdout=output, stderr=subprocess.STDOUT,
                                     timeout=600 if list_only else None)
    finally:
        leftovers = any(temporary.iterdir())
        if leftovers:
            destination = directory / 'tmp'
            if destination.exists() or destination.is_symlink():
                raise RuntimeError(f'refusing to overwrite retained evidence: {destination}')
            temporary.rename(destination)
        else:
            temporary.rmdir()
    result = {'target': name, 'exit_code': process.returncode,
              'seconds': time.monotonic() - start, 'log': str(directory / 'output.log'),
              'cwd': cwd}
    if process.returncode == 0 and leftovers:
        result['exit_code'] = 1
        result['error'] = 'test left temporary files behind; retained beside its output log'
    # Do not print process output: tests can deliberately emit canaries on failure.
    raw = (directory / 'output.log').read_bytes()
    if not list_only and process.returncode == 0 and target.cases is not None:
        summary = re.findall(
            rb'^test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; '
            rb'(\d+) measured; (\d+) filtered out;', raw, re.MULTILINE)
        if not summary or tuple(map(int, summary[-1][:4])) != (len(target.cases), 0, 0, 0):
            result['exit_code'] = 1
            result['error'] = 'libtest did not pass every planned case, or ignored/measured cases were present'
        else:
            result['cases_passed'] = int(summary[-1][0])
        result['cases'] = target.cases
    if process.returncode == 0 and any(canary in raw for canary in [
        b'AKIAEXAMPLEKEYID01', b'wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY',
        b'OPEN_COMPUTE_TEST_WORKERD_TOKEN', b'x-open-compute-generation-token',
    ]):
        result['exit_code'] = 1
        result['error'] = 'process output contains a secret canary; inspect retained evidence locally'
    phase = 'prepare ' if list_only else ''
    print(f'{phase}{name}: {"PASS" if result["exit_code"] == 0 else "FAIL"} {result["seconds"]:.2f}s', flush=True)
    return result


def run_round(targets, artifacts, directory, jobs, execute=execute_target):
    """Stop submitting after a failure; let already running isolated targets clean up."""
    directory.mkdir()
    results = []
    pending = iter(targets)
    waiting = next(pending, None)
    running = {}
    with concurrent.futures.ThreadPoolExecutor(max_workers=jobs) as pool:
        failed = False
        while waiting is not None or running:
            while not failed and waiting is not None and len(running) < jobs:
                exclusive = targets[waiting].exclusive
                if running and (exclusive or any(targets[name].exclusive for name in running.values())):
                    break
                name = waiting
                future = pool.submit(execute, name, artifacts[name], directory / name, targets[name])
                running[future] = name
                waiting = next(pending, None)
                if exclusive:
                    break
            if not running:
                break
            done, _ = concurrent.futures.wait(running, return_when=concurrent.futures.FIRST_COMPLETED)
            for future in done:
                name = running.pop(future)
                try:
                    result = future.result()
                except Exception as error:
                    result = {'target': name, 'exit_code': 1, 'error': str(error)}
                results.append(result)
                failed |= result['exit_code'] != 0
            if failed:
                waiting = None
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('targets', nargs='*', help='Gate targets/groups; use --list to inspect the plan')
    parser.add_argument('--workspace', action='store_true',
                        help='all Cargo test executables once; final mode repeats only product timing cases')
    parser.add_argument('--jobs', type=int, default=DEFAULT_JOBS,
                        help='audited independent processes (default: min(4, CPU count))')
    parser.add_argument('--list', action='store_true', help='validate and print the exact plan without building or running')
    args = parser.parse_args()
    rounds = rounds_from_env()
    validate_registry(TARGETS)
    if args.workspace and args.targets:
        raise ValueError('--workspace cannot be combined with Gate targets')
    flags = ' '.join(os.environ.get(name, '') for name in [
        'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', '__CARGO_LLVM_COV_RUSTC_WRAPPER_RUSTFLAGS'])
    if rounds == 3 and (os.environ.get('CARGO_LLVM_COV') or 'instrument-coverage' in flags):
        raise ValueError('final timing rounds require uninstrumented executables; coverage runs once')
    if not args.workspace and not args.targets:
        raise ValueError('select Gate targets or --workspace')
    if not 1 <= args.jobs <= (os.cpu_count() or 1):
        raise ValueError('--jobs must be between 1 and the host CPU count')
    selected = selection(args.targets)
    targets = resolve_targets(selected, args.workspace)
    plans = round_plan(targets, rounds)
    plan = {'rounds': rounds, 'jobs': args.jobs, 'workspace': args.workspace, 'targets': list(targets),
            'purpose': 'final' if rounds == 3 else 'development',
            'repetition_policy': 'complete-once-timing-three', 'round_plan': plan_summary(plans),
            'exclusive': [name for name, target in targets.items() if target.exclusive],
            'preparation_processes': len(targets),
            'test_processes': sum(len(planned) for planned in plans),
            'inventory_verified': False}
    if args.list:
        print(json.dumps(plan, indent=2))
        return 0
    inputs = verify_inputs()
    source = source_identity()
    run_id = f'{time.strftime("%Y%m%dT%H%M%S")}-{uuid.uuid4().hex[:8]}'
    directory = ROOT / '.temp/gate-run' / run_id
    directory.mkdir(parents=True, mode=0o700)
    report = {**plan, 'source_sha256': source,
        'source_scope': 'repository inputs including docs/references; excludes unconsumed docs plans/history',
        'revision': subprocess.check_output(
        ['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(), 'inputs': inputs,
        'host': platform.platform(), 'cpus': os.cpu_count(),
        'rustc': subprocess.check_output(['rustc', '-Vv'], text=True).strip(),
        'rustflags': os.environ.get('RUSTFLAGS', ''),
        'encoded_rustflags': os.environ.get('CARGO_ENCODED_RUSTFLAGS', '').split('\x1f'),
        'coverage_rustflags': os.environ.get('__CARGO_LLVM_COV_RUSTC_WRAPPER_RUSTFLAGS', '').split('\x1f'),
        'results': [], 'status': 'failed'}
    start = time.monotonic()
    cpu_start = resource.getrusage(resource.RUSAGE_CHILDREN)
    try:
        artifacts, report['build'] = build_targets(targets, directory, args.workspace)
        preparation_start = time.monotonic()
        prepared = run_round(
            {name: target._replace(exclusive=False) for name, target in targets.items()},
            artifacts, directory / 'prepare', args.jobs, partial(execute_target, list_only=True))
        report['preparation'] = {'seconds': time.monotonic() - preparation_start,
                                 'processes_executed': len(prepared), 'targets': prepared}
        if len(prepared) != len(targets) or any(result['exit_code'] for result in prepared):
            raise RuntimeError('test harness preparation failed; no product tests started')
        targets = verify_case_inventory(targets, prepared)
        plans = round_plan(targets, rounds)
        report['round_plan'] = plan_summary(plans)
        report['inventory_verified'] = True
        report['test_cases'] = sum(len(target.cases) for planned in plans for target in planned.values())
        for number, planned in enumerate(plans, 1):
            if source_identity() != source or verify_inputs(probe_version=False) != inputs:
                raise RuntimeError('source or verified inputs changed during the Gate')
            print(f'Gate round {number}/{len(plans)}; jobs={args.jobs}; targets={len(planned)}', flush=True)
            round_start = time.monotonic()
            results = run_round(planned, artifacts, directory / f'round-{number}', args.jobs)
            report['results'].append({'round': number, 'seconds': time.monotonic() - round_start,
                                      'targets': results})
            if len(results) != len(planned) or any(result['exit_code'] for result in results):
                raise RuntimeError('Gate failed; no retries or subsequent rounds')
        if source_identity() != source or verify_inputs(probe_version=False) != inputs:
            raise RuntimeError('source or verified inputs changed during the Gate')
        report['status'] = 'passed'
    except (Exception, KeyboardInterrupt) as error:
        report['error'] = str(error)
    finally:
        report['seconds'] = time.monotonic() - start
        cpu_end = resource.getrusage(resource.RUSAGE_CHILDREN)
        report['child_cpu_seconds'] = cpu_end.ru_utime + cpu_end.ru_stime - cpu_start.ru_utime - cpu_start.ru_stime
        report['test_processes_executed'] = sum(len(r['targets']) for r in report['results'])
        report['test_cases_passed'] = sum(result.get('cases_passed', 0)
                                          for r in report['results'] for result in r['targets'])
        if report['status'] != 'passed':
            failed = ROOT / '.temp/gate-run/failed'
            failed.mkdir(exist_ok=True)
            destination = failed / run_id
            if destination.exists():
                raise RuntimeError(f'refusing to overwrite retained evidence: {destination}')
            directory.rename(destination)
            # Keep report paths accurate after retaining the failed run.
            report = json.loads(json.dumps(report).replace(str(directory), str(destination)))
            directory = destination
        (directory / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        print(f'Gate {report["status"]}: {report["seconds"]:.2f}s; report={directory / "report.json"}', flush=True)
        if 'error' in report:
            print(report['error'], file=sys.stderr)
    return 0 if report['status'] == 'passed' else 1


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
