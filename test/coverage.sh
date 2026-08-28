#!/bin/sh
# Generate Rust coverage reports and enforce the workspace line-coverage gate.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
report_dir="$root/target/llvm-cov"
# Dedicated tests and explicit test-support fixtures are not production Rust.
# Production modules must never be placed behind one of these filename rules.
ignore_filename_regex='/rustlib/src/rust/|/tests/|/src/tests\.rs$|/src/.*_tests\.rs$|/src/mock_s3\.rs$|/src/bin/supervisor_fixture\.rs$'
minimum_lines=90.00
workerd=${OPEN_COMPUTE_TEST_WORKERD:-}
cargo_bin=${CARGO:-cargo}

if ! "$cargo_bin" llvm-cov --version >/dev/null 2>&1; then
  echo "cargo-llvm-cov is required; install it with 'brew install cargo-llvm-cov' or 'cargo install cargo-llvm-cov --locked'" >&2
  exit 1
fi

if [ -z "$workerd" ]; then
  case "$(uname -s)-$(uname -m)" in
    Darwin-arm64|Darwin-x86_64|Linux-x86_64|Linux-aarch64|Linux-arm64)
      workerd="$root/.temp/runtime-cache/v1.20260826.1/workerd" ;;
    *) workerd="" ;;
  esac
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
"$cargo_bin" llvm-cov clean --workspace
"$cargo_bin" llvm-cov \
  --workspace \
  --all-targets \
  --all-features \
  --no-report \
  -- \
  --test-threads=1

mkdir -p "$report_dir"
"$cargo_bin" llvm-cov report --ignore-filename-regex "$ignore_filename_regex" --lcov --output-path "$report_dir/lcov.info"
"$cargo_bin" llvm-cov report --ignore-filename-regex "$ignore_filename_regex" --summary-only --json --output-path "$report_dir/summary.json"
"$cargo_bin" llvm-cov report --ignore-filename-regex "$ignore_filename_regex" --html --output-dir "$report_dir"
"$cargo_bin" llvm-cov report \
  --ignore-filename-regex "$ignore_filename_regex" \
  --fail-under-lines "$minimum_lines"

echo "coverage reports:"
echo "  HTML: $report_dir/html/index.html"
echo "  LCOV: $report_dir/lcov.info"
echo "  JSON: $report_dir/summary.json"
echo "  minimum line coverage: $minimum_lines%"
