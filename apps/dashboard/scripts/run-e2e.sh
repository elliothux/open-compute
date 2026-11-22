#!/bin/sh
# Run dashboard Playwright e2e against a live ocd instance.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
dashboard="$root/apps/dashboard"
if [ "${1:-}" = "--external-proxy" ]; then
  if [ "$#" -ne 4 ] || [ ! -x "$2" ] || [ ! -x "$3" ] || [ ! -x "$4" ]; then
    echo "Usage: $0 --external-proxy <test-support-ocd> <caddy> <certutil>" >&2
    exit 2
  fi
  case "$2" in
    /*) OPEN_COMPUTE_OCD_BIN=$2 ;;
    *) OPEN_COMPUTE_OCD_BIN=$PWD/$2 ;;
  esac
  case "$3" in
    /*) OPEN_COMPUTE_BUILD_CADDY=$3 ;;
    *) OPEN_COMPUTE_BUILD_CADDY=$PWD/$3 ;;
  esac
  case "$4" in
    /*) OPEN_COMPUTE_DASHBOARD_E2E_CERTUTIL=$4 ;;
    *) OPEN_COMPUTE_DASHBOARD_E2E_CERTUTIL=$PWD/$4 ;;
  esac
  export OPEN_COMPUTE_OCD_BIN OPEN_COMPUTE_BUILD_CADDY OPEN_COMPUTE_DASHBOARD_E2E_CERTUTIL
  export OPEN_COMPUTE_DASHBOARD_E2E_EXTERNAL_PROXY=1
  # dev-test.sh provisions the isolated instance with its checked-in dev token.
  unset OPEN_COMPUTE_ADMIN_TOKEN OPEN_COMPUTE_TEST_BROWSER OPEN_COMPUTE_DASHBOARD_E2E_BROWSER_CHANNEL
  cd "$dashboard"
  exec ./node_modules/.bin/playwright test lifecycle.spec.ts --grep 'external proxy Worker origin'
fi

: "${OPEN_COMPUTE_ADMIN_TOKEN:=dev-admin-token}"
: "${OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL:=http://127.0.0.1:8787/operator/}"

export OPEN_COMPUTE_ADMIN_TOKEN
export OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL

attempt=0
until curl -sf "${OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL%/}/../health/live" >/dev/null 2>&1; do
  attempt=$((attempt + 1))
  if [ "$attempt" -ge 120 ]; then
    echo "dashboard e2e: ocd is not reachable at ${OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL}" >&2
    echo "dashboard e2e: start it first with ./scripts/dev-test.sh run" >&2
    exit 1
  fi
  sleep 0.5
done

attempt=0
until curl -sf "${OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL}" >/dev/null 2>&1; do
  attempt=$((attempt + 1))
  if [ "$attempt" -ge 120 ]; then
    echo "dashboard e2e: ocd is live but the embedded dashboard did not become ready" >&2
    curl --silent --show-error --include --max-time 2 \
      "${OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL}" >&2 || true
    exit 1
  fi
  sleep 1
done

cd "$dashboard"
exec bunx playwright test "$@"
