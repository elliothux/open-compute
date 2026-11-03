#!/bin/sh
# Generate Rust coverage reports and enforce the workspace line-coverage gate.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
report_dir="$root/target/llvm-cov"
latest_report_dir="$report_dir"
# Dedicated tests, explicit test-support fixtures, vendored dependencies, and
# the release-only search benchmark are not workspace production Rust.
# Production modules must never be placed behind one of these filename rules.
ignore_filename_regex='/rustlib/src/rust/|^/rustc/|/\.cargo/(registry|git)/|/\.rustup/toolchains/|/third_party/|/tests?/|/src/tests\.rs$|/src/.*_tests\.rs$|/src/mock_s3\.rs$|/src/bin/(s3_fixture|s3_provider_qualification|supervisor_fixture|install_upgrade_fixture)\.rs$|/src/bin/host_extension_test_provider/|/crates/search/examples/exact_search_benchmark\.rs$'
minimum_lines=90.00
mode=run
sharded=0
for option do
  case "$option" in --shard|--shard=*) sharded=1 ;; esac
done
if [ "${1:-}" = --help ]; then
  echo 'Usage: test/coverage.sh [--shard i/4] [Gate options]'
  echo '       test/coverage.sh merge RUN_DIR...'
  echo 'Shards retain raw profiles and exact objects; merge enforces 90% with the same exclusions.'
  exit 0
fi
if [ "${1:-}" = merge ]; then
  mode=merge
  shift
  if [ "$#" -eq 0 ]; then
    echo 'merge requires at least one coverage run directory' >&2
    exit 1
  fi
fi
workerd=${OPEN_COMPUTE_TEST_WORKERD:-}
coverage_html=${OPEN_COMPUTE_COVERAGE_HTML:-1}

case "$coverage_html" in
  0|1) ;;
  *) echo "OPEN_COMPUTE_COVERAGE_HTML must be 0 or 1" >&2; exit 1 ;;
esac

if [ "${OPEN_COMPUTE_GATE_ROUNDS:-1}" != 1 ]; then
  echo "coverage runs exactly once; diagnostic timing rounds require uninstrumented executables" >&2
  exit 1
fi

cd "$root"
# Cargo dependency locations follow the configured home, including Docker mounts.
cargo_home_regex=$(python3 - <<'PYTHON'
import os
from pathlib import Path
import re

home = Path(os.environ.get('CARGO_HOME', str(Path.home() / '.cargo')))
locations = sorted({os.path.abspath(home), str(home.resolve())})
print('(' + '|'.join(re.escape(path) for path in locations) + ')')
PYTHON
)
ignore_filename_regex="$ignore_filename_regex|^$cargo_home_regex/(registry|git)/"
if [ "$mode" = run ]; then
  if ! cargo llvm-cov --version >/dev/null 2>&1; then
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

  # Gate compiles with --offline; fetch the locked crate graph while network is allowed.
  cargo fetch --locked
  # Keep the instrumented target dir so Cargo can reuse compiled artifacts.
  # Do not `cargo llvm-cov clean --workspace`; that cargo-cleans the target.
  # Profile names include process/module identity, so parallel processes cannot collide.
  export CARGO_TARGET_DIR="$root/target/llvm-cov-target"
  mkdir -p "$CARGO_TARGET_DIR" "$root/.temp/coverage"
fi
# Keep profiles, merged data and exact objects immutable in a fresh run directory.
mkdir -p "$root/.temp/coverage"
OPEN_COMPUTE_COVERAGE_RUN_DIR=$(mktemp -d "$root/.temp/coverage/run-XXXXXXXX")
export OPEN_COMPUTE_COVERAGE_RUN_DIR
report_dir="$OPEN_COMPUTE_COVERAGE_RUN_DIR/reports"
profile_list="$OPEN_COMPUTE_COVERAGE_RUN_DIR/profraw-list"
object_list="$OPEN_COMPUTE_COVERAGE_RUN_DIR/object-list"
if [ "$mode" = run ]; then
  mkdir -p "$OPEN_COMPUTE_COVERAGE_RUN_DIR/profiles"
  # Use cargo-llvm-cov's external-runner contract in its own existing build cache.
  gate_plan=$(./test/gate.py --workspace --list "$@")
  coverage_env=$(cargo llvm-cov show-env --sh)
  eval "$coverage_env"
  # Merge child-process profiles into LLVM's bounded pool instead of creating one
  # full-size profile per PID; the latter exhausts hosted-runner disks.
  export LLVM_PROFILE_FILE="$OPEN_COMPUTE_COVERAGE_RUN_DIR/profiles/open-compute-%12m.profraw"
  # show-env exports CARGO_LLVM_COV_SHOW_ENV=1 for printing; leave it set and the
  # RUSTC_WRAPPER re-enters show-env. Prefer direct rustc flags over the wrapper:
  # under macOS maxproc pressure the wrapper fan-out fails with EAGAIN on rustc -vV.
  unset CARGO_LLVM_COV_SHOW_ENV
  if [ -n "${__CARGO_LLVM_COV_RUSTC_WRAPPER_RUSTFLAGS:-}" ]; then
    # show-env uses ASCII unit separator (0x1f) between rustc flag tokens.
    cov_flags=$(printf '%s' "$__CARGO_LLVM_COV_RUSTC_WRAPPER_RUSTFLAGS" | tr '\037' ' ')
    export RUSTFLAGS="${cov_flags}${RUSTFLAGS:+ $RUSTFLAGS}"
  fi
  unset RUSTC_WRAPPER
  if [ "${GITHUB_ACTIONS:-}" = true ] && [ "$(uname -s)" = Linux ] \
    && [ "${SCCACHE_GHA_ENABLED:-}" = true ]; then
    export RUSTC_WRAPPER=sccache
  fi
  export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"
  if [ "${OPEN_COMPUTE_EGRESS_FIXTURE_ALLOW_SUDO:-}" = 1 ]; then
    set -- ./test/test-p0-2-egress-linux.sh --workspace "$@"
  else
    set -- ./test/gate.py --workspace "$@"
  fi
  if [ -n "${OPEN_COMPUTE_COVERAGE_IMAGES:-}" ] \
    && printf '%s' "$gate_plan" | python3 -c 'import json,sys; sys.exit("containers" not in json.load(sys.stdin)["targets"])'; then
    # Match Gate's full feature graph, including dev dependencies, so its broker
    # and test objects are reused without recompiling or losing exact profile inputs.
    cargo test --locked --offline --all-features --no-run --workspace --all-targets
    bash test/containers-fixture/with-ci-linux-engine.sh \
      --images "$OPEN_COMPUTE_COVERAGE_IMAGES" \
      --binary "$CARGO_TARGET_DIR/debug/ocd" \
      --root /tmp/oc-ci-coverage \
      --evidence "$PWD/.temp/containers-ci-engine" \
      --command env OPEN_COMPUTE_EGRESS_FIXTURE_ALLOW_SUDO=1 "$@"
  else
    "$@"
  fi

  if [ "$sharded" = 1 ]; then
    echo "retained coverage shard inputs: $OPEN_COMPUTE_COVERAGE_RUN_DIR"
    exit 0
  fi
  set -- "$OPEN_COMPUTE_COVERAGE_RUN_DIR"
fi
# Merge only explicitly supplied runs, never profiles or executables from a cache.
: > "$profile_list"
: > "$object_list"
for input do
  if [ ! -d "$input/profiles" ] || [ ! -d "$input/objects" ]; then
    echo "coverage run requires profiles and exact objects: $input" >&2
    exit 1
  fi
  find "$input/profiles" -type f -name '*.profraw' -print >> "$profile_list"
  find "$input/objects" -type f -print >> "$object_list"
done

mkdir -p "$report_dir"
# External-runner caches retain prior hashed executables. cargo-llvm-cov's
# report subcommand discovers every executable in that cache, so invoke the
# matching Rust toolchain's LLVM tools directly with the exact copied
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

profdata="$OPEN_COMPUTE_COVERAGE_RUN_DIR/merged.profdata"
if [ ! -s "$profile_list" ]; then
  echo "coverage Gate produced no profile data" >&2
  exit 1
fi
"$llvm_profdata" merge -sparse -f "$profile_list" -o "$profdata"

set --
while IFS= read -r object; do
  if [ "$#" -eq 0 ]; then
    set -- "$object"
  else
    set -- "$@" --object "$object"
  fi
done < "$object_list"
if [ "$#" -eq 0 ]; then
  echo "coverage Gate produced no current object inventory" >&2
  exit 1
fi

"$llvm_cov" export --format=lcov --instr-profile="$profdata" \
  --ignore-filename-regex="$ignore_filename_regex" "$@" > "$report_dir/lcov.info"
"$llvm_cov" export --format=text --summary-only --instr-profile="$profdata" \
  --ignore-filename-regex="$ignore_filename_regex" "$@" > "$report_dir/summary.json"
if [ "$coverage_html" = 1 ]; then
  "$llvm_cov" show --format=html --output-dir="$report_dir/html" --instr-profile="$profdata" \
    --ignore-filename-regex="$ignore_filename_regex" "$@" >/dev/null
fi
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

# Keep this run's report with its exact inputs. Preserve the previous conventional
# reports before publishing the newly qualified reports to the documented path.
if [ -e "$latest_report_dir" ] || [ -L "$latest_report_dir" ]; then
  if [ -L "$latest_report_dir" ] || [ ! -d "$latest_report_dir" ]; then
    echo "coverage report path is not an owned directory: $latest_report_dir" >&2
    exit 1
  fi
  cp -pR "$latest_report_dir" "$OPEN_COMPUTE_COVERAGE_RUN_DIR/previous-reports"
fi
mkdir -p "$latest_report_dir"
cp -pR "$report_dir/." "$latest_report_dir/"
report_dir="$latest_report_dir"

echo "retained coverage inputs and reports: $OPEN_COMPUTE_COVERAGE_RUN_DIR"
echo "coverage reports:"
if [ "$coverage_html" = 1 ]; then
  echo "  HTML: $report_dir/html/index.html"
fi
echo "  LCOV: $report_dir/lcov.info"
echo "  JSON: $report_dir/summary.json"
echo "  minimum line coverage: $minimum_lines%"
