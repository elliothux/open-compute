#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
exec mbx run --quiet --locked --offline \
  -p open-compute-artifacts \
  --features test-support \
  --bin open-compute-s3-provider-qualification \
  -- "$@"
