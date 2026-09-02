ALTER TABLE workers ADD COLUMN ownership TEXT NOT NULL DEFAULT 'tenant'
  CHECK(ownership IN ('tenant', 'system'));

DROP INDEX workers_live_name;

CREATE UNIQUE INDEX workers_live_name
ON workers(account_id, name)
WHERE deleted_at_ms IS NULL AND ownership = 'tenant';

CREATE TABLE system_owned_deployments (
  kind TEXT PRIMARY KEY CHECK(kind = 'dashboard'),
  account_id TEXT NOT NULL REFERENCES accounts(id),
  worker_id TEXT NOT NULL REFERENCES workers(id),
  active_deployment_id TEXT REFERENCES worker_deployments(id),
  assets_sha256 BLOB NOT NULL CHECK(length(assets_sha256) = 32),
  updated_at_ms INTEGER NOT NULL
) STRICT;

UPDATE workers
SET ownership = 'system'
WHERE name = 'open-compute-dashboard'
  AND deleted_at_ms IS NULL;

UPDATE worker_routes
SET state = 'tombstoned',
    deleted_at_ms = COALESCE(deleted_at_ms, updated_at_ms)
WHERE worker_id IN (
  SELECT id FROM workers WHERE name = 'open-compute-dashboard' AND ownership = 'system'
)
AND state = 'active';

INSERT OR REPLACE INTO system_owned_deployments (
  kind, account_id, worker_id, active_deployment_id, assets_sha256, updated_at_ms
)
SELECT
  'dashboard',
  account_id,
  id,
  active_deployment_id,
  zeroblob(32),
  updated_at_ms
FROM workers
WHERE name = 'open-compute-dashboard'
  AND ownership = 'system'
  AND deleted_at_ms IS NULL;
