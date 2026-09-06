#!/bin/sh
# Run ocd with repository-local persistent state and the direct Local backend.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
config="$root/scripts/config/dev.toml"
env_file="$root/scripts/config/dev.env"

fail() {
  echo "open-compute dev: $*" >&2
  exit 1
}

[ -f "$config" ] || fail "checked-in development config is missing"
[ -f "$env_file" ] || fail "checked-in development environment is missing"
set -a
. "$env_file"
set +a

if [ "$#" -eq 0 ]; then
  set -- run
fi

cd "$root"
bun run build
exec cargo run -p open-compute-service --bin ocd -- --config "$config" "$@"
