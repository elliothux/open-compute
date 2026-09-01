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
import shutil
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
TypedTarget = namedtuple(
    'TypedTarget',
    'package_id name kind cwd exclusive cases executable argv discovery_argv env_allowlist '
    'timeout resource_class cleanup_owner',
    defaults=[None, (), (), (), 600, 'light', 'runner'],
)
# Only these targets have been audited for independent TempDir/SQLite/S3/port-0 state.
# p0_1 scans global executable staging, so it is an exclusive barrier.
# The current Workflow product target's 16 concurrent MiB results exhausted
# shared kernel socket buffers in a
# parallel measurement. Keep the workload intact and isolate its test process.
CARGO_TARGETS = {
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
    'p3-assets': ('open-compute-service', 'p3_assets_gate', False),
    'p3-services-hard': ('open-compute-service', 'p3_services_hard', False),
    'p3-services-product': ('open-compute-service', 'p3_services_product', False),
    'p3-services-events': ('open-compute-service', 'p3_services_events', False),
    'p3-services-recovery': ('open-compute-service', 'p3_services_recovery', False),
    'p3-cache-images': ('open-compute-service', 'p3_cache_images_gate', False),
    # Finish independent work together before the remaining exclusive barriers.
    'workflow-product': ('open-compute-service', 'workflow_product_gate', True),
    'runtime': ('open-compute-runtime', 'supervisor', True),
    'single-binary': ('open-compute-service', 'single_binary', True),
}
TYPED_TARGETS = {
    'p3-contract': (None, 'p3-contract', False),
    'p3-cf-diff': (None, 'p3-cf-diff', True),
}
TARGETS = {**CARGO_TARGETS, **TYPED_TARGETS}
P3_PRODUCT_TARGETS = [
    'p3-assets', 'p3-services-hard', 'p3-services-product', 'p3-services-events',
    'p3-services-recovery', 'p3-cache-images',
]
GROUPS = {
    'p0': [name for name in CARGO_TARGETS if name.startswith('p0-')],
    'p1': [name for name in CARGO_TARGETS if name.startswith('p1-')],
    'p2': [name for name in CARGO_TARGETS if name.startswith('p2-') or name.startswith('workflow-')],
    'p3': [
        'p3-contract',
        *[name for name in CARGO_TARGETS if name.startswith('p0-')],
        *[name for name in CARGO_TARGETS if name.startswith('p1-')],
        *[name for name in CARGO_TARGETS if name.startswith('p2-') or name.startswith('workflow-')],
        *P3_PRODUCT_TARGETS,
    ],
    'p3-services': [
        'p3-services-hard', 'p3-services-product', 'p3-services-events',
        'p3-services-recovery',
    ],
    'p3-isolation': ['p1-security', *P3_PRODUCT_TARGETS],
    'p3-recovery': [
        'p1-crash', 'p1-snapshot', 'p2-exit', 'workflow-recovery', *P3_PRODUCT_TARGETS,
    ],
    # Queue/Cron share the immutable loader matrix; selecting both never duplicates it.
    'p2-3': ['p0-2'],
    'p1-8': ['p0-7', 'p1-conformance'],
    'workflow': ['workflow-runtime', 'workflow-recovery', 'workflow-product'],
    'all': [*CARGO_TARGETS, 'p3-contract'],
}


def selection(names):
    selected = set()
    for name in names:
        if name not in TARGETS and name not in GROUPS:
            raise ValueError(f'unknown Gate target: {name}')
        selected.update(GROUPS.get(name, [name]))
    return [name for name in TARGETS if name in selected]


def validate_contract_case_mapping():
    """Every contract/member reference must resolve to one audited native runner case."""
    catalog = json.loads((ROOT / 'test/conformance/catalog.json').read_text())
    if not isinstance(catalog, dict):
        raise ValueError('invalid conformance catalog')
    contracts = catalog.get('contracts')
    if catalog.get('schemaVersion') != 1 or not isinstance(contracts, list) or not contracts:
        raise ValueError('invalid conformance catalog')
    registered = {f'{target}::{case}' for target in TARGETS
                  for case in ONCE.get(target, ()) + TIMING.get(target, ())}
    referenced = set()
    for contract in contracts:
        if not isinstance(contract, dict):
            raise ValueError('invalid conformance contract')
        for field in ('positiveCases', 'negativeCases'):
            cases = contract.get(field)
            if not isinstance(cases, list) or not all(isinstance(case, str) for case in cases):
                raise ValueError(f'invalid conformance contract {field}')
            referenced.update(cases)
    evidence = catalog.get('memberEvidence')
    if not isinstance(evidence, list):
        raise ValueError('invalid conformance member evidence')
    for item in evidence:
        if not isinstance(item, dict):
            raise ValueError('invalid conformance member evidence')
        cases = item.get('runtimeCases', [])
        if (not isinstance(cases, list)
                or not all(isinstance(case, str) and case for case in cases)
                or len(cases) != len(set(cases))):
            raise ValueError('invalid conformance member runtime evidence')
        referenced.update(cases)
    missing = referenced - registered
    if missing:
        raise ValueError(f'catalog references unregistered Gate cases: {sorted(missing)}')


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


def discovered_cases(log, kind='cargo-test'):
    """Parse native discovery without building or executing product behavior."""
    raw = Path(log).read_text()
    if kind != 'cargo-test' and kind not in ('lib', 'test', 'bin'):
        try:
            result = json.loads(next(line for line in reversed(raw.splitlines()) if line.strip()))
        except (StopIteration, json.JSONDecodeError) as error:
            raise ValueError(f'invalid typed target inventory: {log}') from error
        cases = result.get('cases') if isinstance(result, dict) else None
        if (not isinstance(result, dict) or result.get('schemaVersion') != 1
                or not isinstance(cases, list)
                or not cases or not all(isinstance(case, str) and case for case in cases)
                or len(cases) != len(set(cases))):
            raise ValueError(f'invalid typed target inventory: {log}')
        return tuple(sorted(cases))
    cases = re.findall(r'^([^\s]+): test$', raw, re.MULTILINE)
    summary = re.findall(r'^(\d+) tests?, (\d+) benchmarks?$', raw, re.MULTILINE)
    if (len(summary) != 1 or tuple(map(int, summary[0])) != (len(cases), 0)
            or len(cases) != len(set(cases))):
        raise ValueError(f'invalid or unsupported libtest inventory: {log}')
    return tuple(sorted(cases))


def verify_case_inventory(targets, prepared):
    """Do not turn a renamed, new, or filtered-out test into successful acceptance."""
    inventories = {
        item['target']: discovered_cases(item['log'], targets[item['target']].kind)
        for item in prepared
    }
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
             for label, (package, name, exclusive) in CARGO_TARGETS.items()}
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
                                      if label in CARGO_TARGETS else None)
    typed = {}
    if workspace or {'p3-contract', 'p3-cf-diff'} & set(selected):
        bun = shutil.which('bun')
        if bun is None:
            raise ValueError('bun is required for typed conformance targets')
        if workspace or 'p3-contract' in selected:
            typed['p3-contract'] = TypedTarget(
                None,
                'p3-contract',
                'bun-test',
                str(ROOT),
                False,
                tuple(sorted(ONCE['p3-contract'] + TIMING.get('p3-contract', ()))),
                bun,
                (str(ROOT / 'test/conformance/check.ts'),),
                ('--list',),
                ('PATH',),
                120,
                'light',
                'conformance-check',
            )
        if 'p3-cf-diff' in selected:
            typed['p3-cf-diff'] = TypedTarget(
                None,
                'p3-cf-diff',
                'cloudflare-differential',
                str(ROOT),
                True,
                tuple(sorted(ONCE['p3-cf-diff'] + TIMING.get('p3-cf-diff', ()))),
                bun,
                (str(ROOT / 'test/conformance/differential.ts'),),
                ('--list',),
                (
                    'PATH', 'HOME', 'OPEN_COMPUTE_CF_MUTATION_ACK',
                    'OPEN_COMPUTE_CF_ACCOUNT_ID', 'OPEN_COMPUTE_CF_ACCOUNT_ALIAS',
                    'OPEN_COMPUTE_CF_WRANGLER', 'CLOUDFLARE_API_TOKEN', 'OPEN_COMPUTE_PLATFORMD',
                    'OPEN_COMPUTE_ENDPOINT', 'OPEN_COMPUTE_ACCOUNT_ID',
                    'OPEN_COMPUTE_ADMIN_TOKEN', 'OPEN_COMPUTE_TEST_RUNTIME_RESTART_ACK',
                ),
                1800,
                'external-exclusive',
                'differential-runner',
            )
    if workspace:
        missing = CARGO_TARGETS.keys() - found.keys()
        if missing:
            raise ValueError(f'Cargo workspace is missing registered Gate targets: {sorted(missing)}')
        # CLI first loads the actual platformd executable before timed runtime probes.
        # Runtime's tight process-fault windows require an exclusive slot even though
        # its hooks/staging are private; the other libraries have passed together.
        # Complete independent jobs together; unaudited/global-state jobs are barriers.
        resolved = dict(sorted(found.items(), key=lambda item: (
            0 if item[0] == 'open-compute-service.test.cli' else 2 if item[1].exclusive else 1,
            item[0])))
        resolved.update(typed)
        return resolved
    found.update(typed)
    missing = set(selected) - found.keys()
    if missing:
        raise ValueError(f'Cargo metadata is missing selected targets: {sorted(missing)}')
    return {name: found[name] for name in selected}


def digest(path):
    with path.open('rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()


def source_identity():
    names = subprocess.check_output(
        ['git', '-c', 'core.excludesFile=/dev/null', 'ls-files', '-z', '--cached', '--others',
         '--exclude-standard'], cwd=ROOT,
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


def verify_selected_inputs(targets, *, probe_version=True):
    """L0 typed checks need only frozen manifests; Cargo product Gates need runtime inputs."""
    if any(not isinstance(target, TypedTarget) for target in targets.values()):
        return verify_inputs(probe_version=probe_version)
    return {
        'baseline_sha256': digest(ROOT / 'test/conformance/baseline.json'),
        'catalog_sha256': digest(ROOT / 'test/conformance/catalog.json'),
        'capabilities_sha256': digest(ROOT / 'share/cloudflare-capabilities.json'),
    }


def build_targets(targets, directory, workspace):
    cargo_targets = {name: target for name, target in targets.items()
                     if not isinstance(target, TypedTarget)}
    artifacts = {name: target.executable for name, target in targets.items()
                 if isinstance(target, TypedTarget)}
    typed_inputs = {
        name: {
            'kind': target.kind,
            'executable': digest(Path(target.executable)),
            'argv': list(target.argv),
            'source_sha256': digest(Path(target.argv[0])),
            'env_allowlist': list(target.env_allowlist),
            'timeout': target.timeout,
            'resource_class': target.resource_class,
            'cleanup_owner': target.cleanup_owner,
        }
        for name, target in targets.items() if isinstance(target, TypedTarget)
    }
    if not cargo_targets:
        return artifacts, {'invocations': 0, 'seconds': 0.0, 'executables': {},
                           'typed_targets': typed_inputs}
    command = [os.environ.get('CARGO', 'cargo'), 'test', '--locked', '--offline',
               '--all-features', '--no-run', '--message-format=json']
    if workspace:
        command += ['--workspace', '--all-targets']
    else:
        for package in dict.fromkeys(target.package_id for target in cargo_targets.values()):
            command += ['-p', package]
        for target in dict.fromkeys(target.name for target in cargo_targets.values()):
            command += ['--test', target]
    start = time.monotonic()
    with (directory / 'build.stderr.log').open('x') as errors:
        result = subprocess.run(command, cwd=ROOT, stdout=subprocess.PIPE, stderr=errors, text=True)
    expected = {(target.package_id, target.name, target.kind): name
                for name, target in cargo_targets.items()}
    with (directory / 'build.jsonl').open('x') as output:
        output.write(result.stdout)
    if result.returncode:
        errors_path = directory / 'build.stderr.log'
        sys.stderr.write(errors_path.read_text(errors='replace'))
        raise RuntimeError(f'build failed; see {errors_path}')
    for line in result.stdout.splitlines():
        item = json.loads(line)
        if item.get('reason') == 'compiler-artifact' and item.get('executable') and item['profile']['test']:
            # `cargo test --all-targets` still emits ordinary binaries in the test
            # profile even when their Cargo target explicitly has `test = false`.
            # They are build inputs, not libtest harnesses and have no case inventory.
            if item['target'].get('test') is False:
                continue
            key = (item['package_id'], item['target']['name'], item['target']['kind'][0])
            if key not in expected:
                raise RuntimeError(f'Cargo produced an unplanned test executable: {key}')
            artifacts[expected[key]] = item['executable']
    missing = cargo_targets.keys() - artifacts.keys()
    if missing:
        raise RuntimeError(f'Cargo did not produce selected Gate executables: {missing}')
    return artifacts, {'invocations': 1, 'seconds': time.monotonic() - start,
                       'executables': {name: digest(Path(artifacts[name])) for name in cargo_targets},
                       'typed_targets': typed_inputs}


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
    if isinstance(target, TypedTarget):
        env = {key: os.environ[key] for key in target.env_allowlist if key in os.environ}
        env.update(TMPDIR=str(temporary), TMP=str(temporary), TEMP=str(temporary))
        # Bun otherwise materializes its transpiler cache under the isolated TMPDIR,
        # which is a harness leak even for read-only discovery and contract checks.
        env['BUN_RUNTIME_TRANSPILER_CACHE_PATH'] = '0'
    else:
        env = dict(os.environ, TMPDIR=str(temporary), TMP=str(temporary), TEMP=str(temporary))
    # Repetition belongs only to this runner, including when a test spawns its own fixture.
    env.pop('OPEN_COMPUTE_GATE_ROUNDS', None)
    start = time.monotonic()
    try:
        with (directory / 'output.log').open('x') as output:
            # Native harness loading can trigger slow host executable assessment. Finish
            # discovery before any product timeout starts; never prewarm product state.
            if isinstance(target, TypedTarget):
                arguments = [*target.argv, *(target.discovery_argv if list_only else ())]
                if not list_only:
                    arguments += [item for case in target.cases or () for item in ('--case', case)]
            else:
                arguments = ['--list'] if list_only else ['--test-threads=1', '--nocapture']
                if not list_only and target.cases:
                    arguments += ['--exact', *target.cases]
            process = subprocess.run([executable, *arguments], cwd=cwd, env=env,
                                     stdout=output, stderr=subprocess.STDOUT,
                                     timeout=600 if list_only else target.timeout
                                     if isinstance(target, TypedTarget) else None)
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
    if (not list_only and process.returncode == 0 and target.cases is not None
            and isinstance(target, TypedTarget)):
        try:
            typed = json.loads(next(line for line in reversed(raw.decode().splitlines()) if line.strip()))
        except (StopIteration, UnicodeDecodeError, json.JSONDecodeError):
            typed = None
        cases = typed.get('cases') if isinstance(typed, dict) else None
        passed = [item.get('id') for item in cases or []
                  if isinstance(item, dict) and item.get('status') == 'passed']
        failed = [item.get('id') for item in cases or []
                  if isinstance(item, dict) and item.get('status') != 'passed']
        if (typed is None or typed.get('schemaVersion') != 1 or typed.get('status') != 'passed'
                or failed or tuple(sorted(passed)) != tuple(sorted(target.cases))):
            result['exit_code'] = 1
            result['error'] = 'typed target did not pass every planned case exactly once'
        else:
            result['cases_passed'] = len(passed)
            if target.kind == 'cloudflare-differential':
                differential = typed.get('differential')
                if not isinstance(differential, dict):
                    result['exit_code'] = 1
                    result['error'] = 'differential target omitted its sanitized result and cleanup report'
                else:
                    diff_path = directory / 'diff-report.json'
                    diff_path.write_text(json.dumps({
                        'schemaVersion': 1,
                        'status': typed['status'],
                        **differential,
                    }, indent=2) + '\n')
                    result['diff_report'] = str(diff_path)
        result['cases'] = target.cases
    elif not list_only and process.returncode == 0 and target.cases is not None:
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


def write_contract_report(directory, report):
    """Project native Gate results onto the frozen catalog without inflating pass counts."""
    baseline_path = ROOT / 'test/conformance/baseline.json'
    catalog_path = ROOT / 'test/conformance/catalog.json'
    catalog = json.loads(catalog_path.read_text())
    passed = set()
    failed_targets = set()
    executed = set()
    differential_reports = []
    for round_result in report.get('results', []):
        for result in round_result.get('targets', []):
            target = result.get('target')
            if not isinstance(target, str):
                continue
            cases = result.get('cases', ())
            if isinstance(cases, (list, tuple)):
                ids = {f'{target}::{case}' for case in cases if isinstance(case, str)}
                executed.update(ids)
                if result.get('exit_code') == 0:
                    passed.update(ids)
                else:
                    failed_targets.add(target)
            elif result.get('exit_code') != 0:
                failed_targets.add(target)
            if isinstance(result.get('diff_report'), str):
                differential_reports.append(result['diff_report'])
    contracts = []
    counts = {'passed': 0, 'failed': 0, 'not_run': 0, 'unsupported': 0, 'blocked': 0}
    for contract in catalog['contracts']:
        evidence = list(dict.fromkeys(contract['positiveCases'] + contract['negativeCases']))
        if contract['status'] == 'blocked':
            outcome = 'blocked'
        elif contract['status'] == 'unsupported':
            outcome = 'unsupported'
        elif evidence and all(case in passed for case in evidence):
            outcome = 'passed'
        elif any(case.split('::', 1)[0] in failed_targets for case in evidence):
            outcome = 'failed'
        else:
            outcome = 'not_run'
        counts[outcome] += 1
        contracts.append({
            'id': contract['id'],
            'product': contract['product'],
            'status': contract['status'],
            'outcome': outcome,
            'cases': [{'id': case, 'result': 'passed' if case in passed else
                       'executed_failed' if case in executed else 'not_run'} for case in evidence],
            'rejectionEvidence': ('passed' if contract['status'] != 'unsupported'
                                  or evidence and all(case in passed for case in evidence)
                                  else 'failed' if any(case.split('::', 1)[0] in failed_targets
                                                       for case in evidence) else 'not_run'),
            'deviations': contract['deviations'],
        })
    unsupported_complete = all(
        item['status'] != 'unsupported' or item['rejectionEvidence'] == 'passed'
        for item in contracts
    )
    supported_complete = (counts['failed'] == 0 and counts['not_run'] == 0
                          and counts['blocked'] == 0 and unsupported_complete)
    unsupported_failed = any(
        item['status'] == 'unsupported' and item['rejectionEvidence'] == 'failed'
        for item in contracts
    )
    local_verdict = ('contract_go' if supported_complete else
                     'no_go' if counts['failed'] or counts['blocked'] or unsupported_failed
                     else 'incomplete')
    differential = 'not_qualified'
    if differential_reports:
        if len(set(differential_reports)) != 1:
            raise ValueError('more than one differential report was produced')
        diff = json.loads(Path(differential_reports[0]).read_text())
        differential = 'qualified' if diff.get('status') == 'passed' else 'failed'
    platform_verdict = ('go' if local_verdict == 'contract_go' and differential == 'qualified'
                        else 'conditional_go' if local_verdict == 'contract_go'
                        and differential == 'not_qualified' else 'no_go'
                        if local_verdict == 'no_go' or differential == 'failed' else 'incomplete')
    result = {
        'schemaVersion': 1,
        'baselineSha256': digest(baseline_path),
        'catalogSha256': digest(catalog_path),
        'sourceSha256': report.get('source_sha256'),
        'localVerdict': local_verdict,
        'cloudflareDifferential': differential,
        'platformVerdict': platform_verdict,
        'counts': counts,
        'contracts': contracts,
    }
    path = directory / 'contract-report.json'
    path.write_text(json.dumps(result, indent=2) + '\n')
    report['contract_report'] = str(path)
    report['contract_verdict'] = result['platformVerdict']


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
    validate_contract_case_mapping()
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
    inputs = verify_selected_inputs(targets)
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
            if source_identity() != source or verify_selected_inputs(
                    targets, probe_version=False) != inputs:
                raise RuntimeError('source or verified inputs changed during the Gate')
            print(f'Gate round {number}/{len(plans)}; jobs={args.jobs}; targets={len(planned)}', flush=True)
            round_start = time.monotonic()
            results = run_round(planned, artifacts, directory / f'round-{number}', args.jobs)
            report['results'].append({'round': number, 'seconds': time.monotonic() - round_start,
                                      'targets': results})
            if len(results) != len(planned) or any(result['exit_code'] for result in results):
                raise RuntimeError('Gate failed; no retries or subsequent rounds')
        if source_identity() != source or verify_selected_inputs(
                targets, probe_version=False) != inputs:
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
        try:
            write_contract_report(directory, report)
        except (OSError, ValueError, KeyError, TypeError) as error:
            report['contract_report_error'] = str(error)
            report['status'] = 'failed'
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
