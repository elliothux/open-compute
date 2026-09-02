#!/bin/sh
# Run dashboard Playwright e2e against a live ocd instance.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
dashboard="$root/packages/dashboard"
: "${OPEN_COMPUTE_ADMIN_TOKEN:=dev-admin-token}"
: "${OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL:=http://127.0.0.1:8787/operator/}"

export OPEN_COMPUTE_ADMIN_TOKEN
export OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL

if ! curl -sf "${OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL%/}/../health/live" >/dev/null 2>&1; then
  echo "dashboard e2e: ocd is not reachable at ${OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL}" >&2
  echo "dashboard e2e: start it first with ./scripts/dev-test.sh run" >&2
  exit 1
fi

attempt=0
until curl -sf "${OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL}" >/dev/null 2>&1; do
  attempt=$((attempt + 1))
  if [ "$attempt" -ge 30 ]; then
    echo "dashboard e2e: ocd is live but the embedded dashboard did not become ready" >&2
    exit 1
  fi
  sleep 1
done

cd "$dashboard"
exec bunx playwright test "$@"
