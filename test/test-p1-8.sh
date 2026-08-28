#!/bin/sh
# Record the conditional P1.8 No-Go while preserving the stock-workerd basic WebSocket Gate.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
workerd=${OPEN_COMPUTE_TEST_WORKERD:-"$root/.temp/runtime-cache/v1.20260826.1/workerd"}
if [ ! -f "$workerd" ]; then
  echo "OPEN_COMPUTE_TEST_WORKERD is missing; the P1.8 Gate refuses to skip" >&2
  exit 1
fi
if rg -q 'acceptWebSocket|getWebSockets|serializeAttachment|deserializeAttachment' \
  "$root/runtime/system-workers/durable-objects/facade.js" "$root/runtime/system-workers/durable-objects/host.js"; then
  echo "P1.8 facade unexpectedly exposes an unverified hibernation method" >&2
  exit 1
fi
export OPEN_COMPUTE_TEST_WORKERD="$workerd"
cd "$root"
cargo test -p open-compute-service --features test-support \
  --test p0_7_durable_objects_gate -- --test-threads=1 --nocapture
cargo test -p open-compute-service --features test-support \
  --test p1_conformance -- --test-threads=1
printf 'P1.8 NO_GO workerd=v1.20260826.1 reason=WH-01-facade-absent basic-websocket=passed\n'
