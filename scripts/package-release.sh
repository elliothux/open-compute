#!/bin/sh
# Thin launcher: package a verified offline release layout.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
dest=${DEST:-}
download=0
archive=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --dest) dest=$2; shift 2 ;;
    --download) download=1; shift ;;
    --archive) archive=$2; shift 2 ;;
    *) echo "usage: $0 --dest ABS [--download] [--archive ABS]" >&2; exit 2 ;;
  esac
done
if [ -z "$dest" ]; then
  echo "DEST or --dest is required" >&2
  exit 2
fi
bin="$root/target/release/platformd"
revision=$(git -C "$root" rev-parse --verify HEAD)
if [ -n "$(git -C "$root" status --porcelain --untracked-files=all)" ]; then
  echo "release packaging requires a clean checkout so release.json names exact source" >&2
  exit 1
fi
# Always ask Cargo to validate freshness. Incremental no-op builds are cheap,
# while reusing an arbitrary executable left in target/ can package stale code.
OPEN_COMPUTE_GIT_REVISION="$revision" \
  cargo build -q --release -p open-compute-service --bin platformd
set -- package-release \
  --dest "$dest" \
  --lock "$root/runtime/workerd.lock.json" \
  --assets "$root/runtime" \
  --license "$root/LICENSE" \
  --default-config "$root/share/default-config.toml" \
  --runbooks "$root/docs/runbooks"
if [ "$download" -eq 1 ]; then
  set -- "$@" --download
fi
if [ -n "$archive" ]; then
  set -- "$@" --archive "$archive"
fi
exec "$bin" "$@"
