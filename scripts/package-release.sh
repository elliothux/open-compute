#!/bin/sh
# Produce exactly one native, self-contained platformd executable.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
"$root/node_modules/.bin/tsc" --project "$root/scripts/tsconfig.json" --noEmit
exec bun "$root/scripts/package-release.ts" "$@"
