#!/bin/sh
# Deterministic local aggregate for P1.0-P1.8 plus the complete stock-workerd P0 regression.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
./test/test-p1-conformance.sh
./test/test-p1-security.sh
./test/test-p1-crash.sh
./test/test-p1-upgrade.sh
./test/test-p1-8.sh
cargo test -p open-compute-service --features test-support \
  --test p1_reliability -- --test-threads=1
./test/load-p1.sh --profile mixed --iterations 3
./test/test-p0-exit.sh
printf 'P1 aggregate PASS\n'
