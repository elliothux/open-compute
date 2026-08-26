#!/bin/sh
# Run P1 parser/path, isolation, malicious-Worker, fuzz, canary, and artifact-hygiene Gates.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
workerd=${OPEN_COMPUTE_TEST_WORKERD:-"$root/poc/.runtime-cache/v1.20260826.1/workerd"}
if [ ! -f "$workerd" ]; then
  echo "OPEN_COMPUTE_TEST_WORKERD is missing; the P1 security Gate refuses to skip" >&2
  exit 1
fi
export OPEN_COMPUTE_TEST_WORKERD="$workerd"
cd "$root"
cargo test -p open-compute-core --all-features snapshot_manifest -- --test-threads=1
cargo test -p open-compute-storage --all-features p1_offline_snapshot -- --test-threads=1
cargo test -p open-compute-storage --all-features \
  worker_repository_rejects_invalid_state_and_ownership_operations -- --test-threads=1
cargo test -p open-compute-storage --all-features \
  authorizer_blocks_cross_database_internal_and_connection_state_sql -- --test-threads=1
cargo test -p open-compute-storage --all-features \
  authority_rejects_cross_namespace_stale_generation_and_live_namespace_delete -- --test-threads=1
cargo test -p open-compute-service --features test-support \
  --test p1_security -- --test-threads=1
cargo test -p open-compute-service --all-features --lib \
  tenant_headers_strip_forged_identity_and_hop_by_hop -- --test-threads=1
security_run=$(/usr/bin/mktemp -d "${TMPDIR:-/tmp}/open-compute-p1-security.XXXXXX")
security_log="$security_run/p0-exit.log"
preserve_security_run=1
cleanup_security_run() {
  if [ "$preserve_security_run" -eq 0 ]; then
    /bin/rm -R "$security_run"
  else
    echo "P1 security failure evidence preserved at $security_run" >&2
  fi
}
trap cleanup_security_run EXIT HUP INT TERM
if ! cargo test -p open-compute-service --features test-support \
  --test p0_exit_gate -- --test-threads=1 --nocapture >"$security_log" 2>&1; then
  /bin/cat "$security_log"
  exit 1
fi
/bin/cat "$security_log"
if LC_ALL=C grep -aE \
  'AKIAEXAMPLEKEYID01|wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY|OPEN_COMPUTE_TEST_WORKERD_TOKEN|x-open-compute-generation-token' \
  "$security_log" >/dev/null; then
  echo "P1 process output contains a secret canary" >&2
  exit 1
fi
cargo test -p open-compute-service --all-features \
  p1_capability_release_support_bundle_and_metrics_contract_is_bounded -- --test-threads=1
./scripts/fuzz-p1.sh --seconds 10

cargo build --release -p open-compute-service --bin platformd
if LC_ALL=C grep -aE \
  'fault-injection-route|x-open-compute-crash-after|OPEN_COMPUTE_DISABLE_AUTH|OPEN_COMPUTE_SKIP_RUNTIME_VERIFY|p1-to-json-trap|AKIAEXAMPLEKEYID01' \
  target/release/platformd >/dev/null; then
  echo "production platformd contains a P1 test-only marker or credential canary" >&2
  exit 1
fi
preserve_security_run=0
printf 'P1 security PASS\n'
