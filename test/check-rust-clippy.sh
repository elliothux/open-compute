#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

./test/check-source-policy.sh

production_clippy() {
    cargo clippy "$@" --no-default-features --keep-going -- \
        -D warnings \
        -D clippy::unwrap_used \
        -D clippy::expect_used \
        -D clippy::panic \
        -D clippy::todo \
        -D clippy::unimplemented
}

# Lint production libraries together without enabling test-support features.
production_status=0
# All workspace libraries use the same production feature set. One Cargo
# invocation shares dependency compilation; --no-deps keeps Clippy from
# traversing third-party sources while still compiling them as inputs.
production_clippy --workspace --lib --no-deps || production_status=1

# The benchmark and daemon are the maintained non-test executable targets.
production_clippy -p open-compute-search --example exact_search_benchmark --no-deps || production_status=1
production_clippy -p open-compute-service --bin ocd --no-deps || production_status=1

if [ "$production_status" -ne 0 ]; then
    exit "$production_status"
fi

# Crate-local and integration tests use the dedicated 800-line function budget.
CLIPPY_CONF_DIR="$root/test/clippy" \
    cargo clippy --workspace --tests --all-features --no-deps --keep-going -- -D warnings

# Test-support fixture and fuzz binaries are not selected by `--tests`.
CLIPPY_CONF_DIR="$root/test/clippy" \
    cargo clippy \
        -p open-compute-artifacts \
        -p open-compute-runtime \
        -p open-compute-service \
        -p open-compute-p1-fuzz \
        --bins --no-deps \
        --all-features --keep-going -- -D warnings
