#!/usr/bin/env python3
"""Build the production feature set once and reject test hooks in the executable."""
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
# Fault markers and fixture payloads belong to tests, never production artifacts.
MARKERS = (
    b'fault-injection-route', b'x-open-compute-crash-after', b'OPEN_COMPUTE_DISABLE_AUTH',
    b'OPEN_COMPUTE_SKIP_RUNTIME_VERIFY', b'p1-to-json-trap', b'AKIAEXAMPLEKEYID01',
    b'OPEN_COMPUTE_P2_2_FAULT',
    b'matrix-json-body', b'runtime-gate-throw', b'runtime-gate-wait-until',
    b'runtime-gate-timeout', b'p23-first', b'p23-second', b'p23-third',
    *(f'QG-{number:02}'.encode() for number in range(1, 11)),
)
MARKER_GROUPS = ((
    b'AfterClaimCommit', b'BeforeDispatch', b'AfterDispatchBeforeComplete',
    b'AfterCompleteCommit', b'DuringProjectionRefresh',
),)


def main():
    result = subprocess.run([os.environ.get('CARGO', 'cargo'), 'build', '--locked', '--offline',
                             '--no-default-features', '-p', 'open-compute-service', '--bin',
                             'ocd', '--message-format=json'], cwd=ROOT,
                            stdout=subprocess.PIPE, text=True, check=True)
    binaries = [item['executable'] for line in result.stdout.splitlines()
                if (item := json.loads(line)).get('reason') == 'compiler-artifact'
                and item.get('executable') and item['target']['name'] == 'ocd']
    if len(binaries) != 1:
        raise SystemExit('Cargo did not produce exactly one production ocd')
    data = Path(binaries[0]).read_bytes()
    if (any(marker in data for marker in MARKERS)
            or any(all(marker in data for marker in group) for group in MARKER_GROUPS)):
        raise SystemExit('production executable contains a test marker; no release packaging was performed')
    subprocess.run(['bun', 'scripts/verify-release-executable.ts', binaries[0],
                    os.environ.get('OPEN_COMPUTE_GIT_REVISION', 'unknown')],
                   cwd=ROOT, check=True)
    print('production feature/artifact hygiene and release CLI PASS (one ordinary build; no packaging)')


if __name__ == '__main__':
    main()
