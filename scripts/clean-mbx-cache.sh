#!/bin/sh
# Collect the global shared cache; accepts --dry-run and --max-size SIZE.
set -eu
exec mbx gc "$@"
