#!/bin/sh
# Persistent repository-local development profile of the scoped test-support daemon.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
export OPEN_COMPUTE_DEV_STATE_DIR=$root/.data/open-compute/dev
export OPEN_COMPUTE_DEV_OCD_ROOT=$root/.data/r1
export OPEN_COMPUTE_DEV_PORT=8787
exec "$root/scripts/dev-test.sh" "$@"
