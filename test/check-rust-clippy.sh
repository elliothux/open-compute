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

# Lint each production library independently so workspace dev-dependency feature
# unification cannot pull test-support modules into the production pass.
production_status=0
for package in \
    open-compute-core \
    open-compute-storage \
    open-compute-search \
    open-compute-artifacts \
    open-compute-runtime \
    open-compute-images \
    open-compute-document-parser \
    open-compute-workers \
    open-compute-service
do
    production_clippy -p "$package" --lib || production_status=1
done

# The benchmark and daemon are the maintained non-test executable targets.
production_clippy -p open-compute-search --example exact_search_benchmark || production_status=1
production_clippy -p open-compute-service --bin ocd || production_status=1

if [ "$production_status" -ne 0 ]; then
    exit "$production_status"
fi

# Crate-local and integration tests use the dedicated 800-line function budget.
CLIPPY_CONF_DIR="$root/test/clippy" \
    cargo clippy --workspace --tests --all-features --keep-going -- -D warnings

# Test-support fixture and fuzz binaries are not selected by `--tests`.
CLIPPY_CONF_DIR="$root/test/clippy" \
    cargo clippy \
        -p open-compute-artifacts \
        -p open-compute-runtime \
        -p open-compute-p1-fuzz \
        --bins \
        --all-features --keep-going -- -D warnings
