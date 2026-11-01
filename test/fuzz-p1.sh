#!/bin/sh
# Run the independent deterministic P1 parser fuzz budget without downloads or uploads.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
seconds=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --seconds) seconds=$2; shift 2 ;;
    *) echo "usage: $0 --seconds 1..3600" >&2; exit 2 ;;
  esac
done
case "$seconds" in ''|*[!0-9]*|0) echo "seconds must be 1..3600" >&2; exit 2 ;; esac
if [ "$seconds" -gt 3600 ]; then
  echo "seconds must be 1..3600" >&2
  exit 2
fi
cd "$root"
cargo run --offline --manifest-path test/fuzz/Cargo.toml --release -- --seconds "$seconds"
