#!/bin/sh
# Generate runtime stress fixtures under .temp/stress-data/ (never committed).
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
data_dir="${root}/.temp/stress-data"
run_stamp=$(date -u +%Y%m%dT%H%M%SZ)

mkdir -p "$data_dir"

DATA_DIR="$data_dir" RUN_STAMP="$run_stamp" python3 - <<'PY'
import json
import os
from pathlib import Path

data_dir = Path(os.environ["DATA_DIR"])
run_stamp = os.environ["RUN_STAMP"]

keys = [f"stress-kv-{run_stamp}-{index}" for index in range(1, 51)]
queue_templates = [
    {"label": f"stress-queue-{run_stamp}-{index}", "payload": {"index": index, "phase": "seed"}}
    for index in range(1, 21)
]
r2_specs = [
    {"key": f"stress-r2-{run_stamp}-{index}", "bytes": 256 * (index % 5 + 1)}
    for index in range(1, 11)
]
d1_seed_sql = "\n".join(
    [
        "CREATE TABLE IF NOT EXISTS orders (id TEXT PRIMARY KEY, status TEXT NOT NULL, revision TEXT NOT NULL, payload_bytes INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL);",
        "CREATE INDEX IF NOT EXISTS idx_orders_status_updated ON orders(status, updated_at DESC);",
        "CREATE TABLE IF NOT EXISTS idempotency (key TEXT PRIMARY KEY, order_id TEXT NOT NULL, response_json TEXT NOT NULL, created_at INTEGER NOT NULL);",
        "CREATE TABLE IF NOT EXISTS inventory_events (object_id TEXT NOT NULL, delta INTEGER NOT NULL, revision TEXT NOT NULL, created_at INTEGER NOT NULL);",
        "CREATE INDEX IF NOT EXISTS idx_inventory_events_object ON inventory_events(object_id, created_at DESC);",
    ]
)

(data_dir / "keys.txt").write_text("\n".join(keys) + "\n", encoding="utf-8")
(data_dir / "d1-seed.sql").write_text(d1_seed_sql + "\n", encoding="utf-8")
(data_dir / "queue-templates.json").write_text(json.dumps(queue_templates, indent=2), encoding="utf-8")
(data_dir / "r2-specs.json").write_text(json.dumps(r2_specs, indent=2), encoding="utf-8")
(data_dir / "manifest.json").write_text(
    json.dumps(
        {
            "generated_at": run_stamp,
            "keys": len(keys),
            "queue_templates": len(queue_templates),
            "r2_specs": len(r2_specs),
            "paths": {
                "keys": "keys.txt",
                "d1_seed": "d1-seed.sql",
                "queue_templates": "queue-templates.json",
                "r2_specs": "r2-specs.json",
            },
        },
        indent=2,
    ),
    encoding="utf-8",
)
print(f"generated stress data in {data_dir}")
PY
