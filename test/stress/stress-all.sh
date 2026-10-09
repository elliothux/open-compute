#!/bin/sh
# Orchestrate smoke -> per-stack P0 -> scenario mega-checkout.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
port=8788
run_p1=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --port)
      port=$2
      shift 2
      ;;
    --p1)
      run_p1=1
      shift
      ;;
    *)
      echo "usage: stress-all.sh [--port 8788] [--p1]" >&2
      exit 1
      ;;
  esac
done

export STRESS_BASE_URL="http://127.0.0.1:${port}"
export OC_PUBLIC_PORT="$port"

# shellcheck disable=SC1091
. "${root}/test/stress/common.sh"
require_disk_space 5 "${root}/.temp"

if [ -f "${root}/.temp/stress-run/.deploy_env" ]; then
  set -a
  # shellcheck disable=SC1091
  . "${root}/.temp/stress-run/.deploy_env"
  set +a
fi

if [ -z "${STRESS_ACCOUNT_ID:-}" ]; then
  echo "STRESS_ACCOUNT_ID is required; run deploy-stress-demo.sh first" >&2
  exit 1
fi

bash "${root}/test/stress/stress-smoke.sh"
bash "${root}/test/stress/stress-p0.sh"
bash "${root}/test/stress/stress-scenario.sh"

if [ "$run_p1" -eq 1 ]; then
  bash "${root}/test/stress/stress-p1.sh"
  bash "${root}/test/stress/stress-p1-soak.sh"
fi

printf 'stress-all complete against %s\n' "$STRESS_BASE_URL"
