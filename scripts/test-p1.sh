#!/bin/sh
# Deterministic local aggregate for P1.0-P1.8 plus the complete stock-workerd P0 regression.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
./scripts/test-p1-conformance.sh
./scripts/test-p1-security.sh
./scripts/test-p1-crash.sh
./scripts/test-p1-upgrade.sh
./scripts/test-p1-8.sh
cargo test -p open-compute-service --features test-support \
  --test p1_reliability -- --test-threads=1
./scripts/load-p1.sh --profile mixed --iterations 3
./scripts/test-p0-exit.sh
printf 'P1 aggregate PASS\n'
