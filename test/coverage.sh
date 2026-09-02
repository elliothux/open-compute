#!/bin/sh
# Generate Rust coverage reports and enforce the workspace line-coverage gate.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
report_dir="$root/target/llvm-cov"
# Dedicated tests and explicit test-support fixtures are not production Rust.
# Production modules must never be placed behind one of these filename rules.
ignore_filename_regex='/rustlib/src/rust/|^/rustc/|/\.cargo/(registry|git)/|/\.rustup/toolchains/|/tests?/|/src/tests\.rs$|/src/.*_tests\.rs$|/src/mock_s3\.rs$|/src/bin/(s3_fixture|supervisor_fixture)\.rs$'
minimum_lines=90.00
workerd=${OPEN_COMPUTE_TEST_WORKERD:-}
cargo_bin=${CARGO:-cargo}

if [ "${OPEN_COMPUTE_GATE_ROUNDS:-1}" != 1 ]; then
  echo "coverage runs exactly once; final timing rounds require uninstrumented executables" >&2
  exit 1
fi

if ! "$cargo_bin" llvm-cov --version >/dev/null 2>&1; then
  echo "cargo-llvm-cov is required; install it with 'brew install cargo-llvm-cov' or 'cargo install cargo-llvm-cov --locked'" >&2
  exit 1
fi

if [ -z "$workerd" ] || [ ! -f "$workerd" ]; then
  echo "OPEN_COMPUTE_TEST_WORKERD is missing; coverage requires the real P0 Gates" >&2
  exit 1
fi
case "$workerd" in
  /*) ;;
  *) echo "OPEN_COMPUTE_TEST_WORKERD must be absolute" >&2; exit 1 ;;
esac
export OPEN_COMPUTE_TEST_WORKERD="$workerd"

cd "$root"
# Gate compiles with --offline; fetch the locked crate graph while network is allowed.
"$cargo_bin" fetch --locked
# Keep the instrumented target dir so rust-cache can reuse compiled artifacts.
# Do not `cargo llvm-cov clean --workspace`; that cargo-cleans the target.
# Profile names include process/module identity, so parallel processes cannot collide.
export CARGO_TARGET_DIR="$root/target/llvm-cov-target"
mkdir -p "$CARGO_TARGET_DIR"
find "$CARGO_TARGET_DIR" \( -name '*.profraw' -o -name '*.profdata' \) -delete
# Use cargo-llvm-cov's external-runner contract in its own existing build cache.
./test/gate.py --workspace --list "$@" >/dev/null
coverage_env=$("$cargo_bin" llvm-cov show-env --sh)
eval "$coverage_env"
./test/gate.py --workspace "$@"

mkdir -p "$report_dir"
# External-runner caches retain prior hashed executables. cargo-llvm-cov's
# report subcommand discovers every executable in that cache, so invoke the
# matching Rust toolchain's LLVM tools directly with the exact hard-linked
# object set emitted by this Gate build.
rustc_bin=${RUSTC:-rustc}
toolchain=$($rustc_bin --print sysroot)
host=$($rustc_bin -vV | sed -n 's/^host: //p')
llvm_cov=${LLVM_COV:-$toolchain/lib/rustlib/$host/bin/llvm-cov}
llvm_profdata=${LLVM_PROFDATA:-$toolchain/lib/rustlib/$host/bin/llvm-profdata}
if [ ! -x "$llvm_cov" ] || [ ! -x "$llvm_profdata" ]; then
  echo "the active Rust toolchain does not provide llvm-cov and llvm-profdata" >&2
  exit 1
fi

profile_list="$CARGO_TARGET_DIR/current-profraw-list"
profdata="$CARGO_TARGET_DIR/current.profdata"
find "$CARGO_TARGET_DIR" -type f -name '*.profraw' -print > "$profile_list"
if [ ! -s "$profile_list" ]; then
  echo "coverage Gate produced no profile data" >&2
  exit 1
fi
"$llvm_profdata" merge -sparse -f "$profile_list" -o "$profdata"

object_dir="$CARGO_TARGET_DIR/current-objects"
set -- "$object_dir"/*
if [ "$1" = "$object_dir/*" ] || [ ! -f "$1" ]; then
  echo "coverage Gate produced no current object inventory" >&2
  exit 1
fi
first_object=$1
shift
set -- "$first_object" "$@"
object_args=$#
set -- "$first_object"
for object in "$object_dir"/*; do
  if [ "$object" != "$first_object" ]; then
    set -- "$@" --object "$object"
  fi
done
if [ "$object_args" -ne "$((($# + 1) / 2))" ]; then
  echo "coverage object inventory changed while generating reports" >&2
  exit 1
fi

"$llvm_cov" export --format=lcov --instr-profile="$profdata" \
  --ignore-filename-regex="$ignore_filename_regex" "$@" > "$report_dir/lcov.info"
"$llvm_cov" export --format=text --summary-only --instr-profile="$profdata" \
  --ignore-filename-regex="$ignore_filename_regex" "$@" > "$report_dir/summary.json"
"$llvm_cov" show --format=html --output-dir="$report_dir/html" --instr-profile="$profdata" \
  --ignore-filename-regex="$ignore_filename_regex" "$@" >/dev/null
python3 - "$report_dir/summary.json" "$minimum_lines" <<'PY'
import json
import sys

summary = json.load(open(sys.argv[1], encoding='utf-8'))
lines = summary['data'][0]['totals']['lines']
minimum = float(sys.argv[2])
if lines['percent'] < minimum:
    raise SystemExit(
        f"workspace line coverage {lines['percent']:.2f}% is below {minimum:.2f}%"
    )
print(f"workspace line coverage: {lines['percent']:.2f}%")
PY

echo "coverage reports:"
echo "  HTML: $report_dir/html/index.html"
echo "  LCOV: $report_dir/lcov.info"
echo "  JSON: $report_dir/summary.json"
echo "  minimum line coverage: $minimum_lines%"
