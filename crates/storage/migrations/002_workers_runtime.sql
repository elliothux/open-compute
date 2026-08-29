CREATE TABLE workers (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  name TEXT NOT NULL,
  active_deployment_id TEXT REFERENCES worker_deployments(id) DEFERRABLE INITIALLY DEFERRED,
  do_storage_id TEXT NOT NULL,
  route_generation INTEGER NOT NULL DEFAULT 0 CHECK(route_generation >= 0),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  deleted_at_ms INTEGER,
  CHECK(length(name) BETWEEN 1 AND 63)
) STRICT;

CREATE UNIQUE INDEX workers_live_name
ON workers(account_id, name)
WHERE deleted_at_ms IS NULL;

CREATE TABLE worker_deployments (
  id TEXT PRIMARY KEY,
  worker_id TEXT NOT NULL REFERENCES workers(id),
  version_number INTEGER NOT NULL CHECK(version_number > 0),
  content_kind TEXT NOT NULL CHECK(content_kind IN ('worker', 'assets_only')),
  state TEXT NOT NULL CHECK(state IN (
    'staging', 'validating', 'ready', 'rejected', 'deleting', 'tombstoned'
  )),
  artifact_sha256 BLOB CHECK(artifact_sha256 IS NULL OR length(artifact_sha256) = 32),
  artifact_size INTEGER CHECK(artifact_size IS NULL OR artifact_size >= 0),
  artifact_schema_version INTEGER,
  main_module TEXT,
  compatibility_date TEXT NOT NULL,
  compatibility_flags_json BLOB NOT NULL,
  limits_json BLOB NOT NULL,
  worker_code_sha256 BLOB NOT NULL CHECK(length(worker_code_sha256) = 32),
  loader_schema_version INTEGER NOT NULL,
  created_at_ms INTEGER NOT NULL,
  ready_at_ms INTEGER,
  rejected_at_ms INTEGER,
  rejection_code TEXT,
  deleted_at_ms INTEGER,
  CHECK(
    (content_kind = 'worker' AND artifact_sha256 IS NOT NULL AND
     artifact_size IS NOT NULL AND artifact_schema_version IS NOT NULL AND
     main_module IS NOT NULL) OR
    (content_kind = 'assets_only' AND artifact_sha256 IS NULL AND
     artifact_size IS NULL AND artifact_schema_version IS NULL AND
     main_module IS NULL)
  ),
  UNIQUE(worker_id, version_number)
) STRICT;

CREATE INDEX deployments_worker_state
ON worker_deployments(worker_id, state, version_number DESC);

CREATE TABLE deployment_vars (
  deployment_id TEXT NOT NULL REFERENCES worker_deployments(id),
  name TEXT NOT NULL,
  value_json BLOB NOT NULL,
  PRIMARY KEY(deployment_id, name)
) WITHOUT ROWID, STRICT;

CREATE TABLE deployment_secrets (
  deployment_id TEXT NOT NULL REFERENCES worker_deployments(id),
  name TEXT NOT NULL,
  revision_id TEXT NOT NULL,
  key_id TEXT NOT NULL,
  algorithm TEXT NOT NULL,
  nonce BLOB NOT NULL,
  ciphertext BLOB NOT NULL,
  PRIMARY KEY(deployment_id, name)
) WITHOUT ROWID, STRICT;

CREATE TABLE worker_routes (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  worker_id TEXT NOT NULL REFERENCES workers(id),
  kind TEXT NOT NULL CHECK(kind IN ('platform_path', 'exact_host')),
  hostname_ascii TEXT,
  path_prefix TEXT NOT NULL,
  entrypoint TEXT,
  state TEXT NOT NULL CHECK(state IN ('active', 'disabled', 'tombstoned')),
  generation INTEGER NOT NULL CHECK(generation > 0),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  deleted_at_ms INTEGER,
  CHECK((kind = 'platform_path' AND hostname_ascii IS NULL) OR
        (kind = 'exact_host' AND hostname_ascii IS NOT NULL))
) STRICT;

CREATE UNIQUE INDEX live_exact_routes
ON worker_routes(account_id, hostname_ascii, path_prefix)
WHERE kind = 'exact_host' AND state = 'active';

CREATE UNIQUE INDEX live_platform_routes
ON worker_routes(account_id, path_prefix)
WHERE kind = 'platform_path' AND state = 'active';

CREATE TABLE control_idempotency (
  account_id TEXT NOT NULL,
  scope TEXT NOT NULL,
  idempotency_key TEXT NOT NULL,
  fingerprint_key_id TEXT NOT NULL,
  request_fingerprint BLOB NOT NULL CHECK(length(request_fingerprint) = 32),
  response_json BLOB,
  deployment_id TEXT REFERENCES worker_deployments(id),
  resource_id TEXT REFERENCES resources(id) DEFERRABLE INITIALLY DEFERRED,
  queue_id TEXT REFERENCES queues(id) DEFERRABLE INITIALLY DEFERRED,
  state TEXT NOT NULL CHECK(state IN ('running', 'complete', 'failed')),
  created_at_ms INTEGER NOT NULL,
  expires_at_ms INTEGER NOT NULL,
  PRIMARY KEY(account_id, scope, idempotency_key)
) WITHOUT ROWID, STRICT;

-- Every subsystem that keeps a deployment reachable registers a typed row
-- here. Deletion consults this registry instead of an incomplete COUNT spread
-- across product-specific tables.
CREATE TABLE deployment_referrers (
  deployment_id TEXT NOT NULL REFERENCES worker_deployments(id),
  kind TEXT NOT NULL,
  ref_id TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  PRIMARY KEY(deployment_id, kind, ref_id)
) WITHOUT ROWID, STRICT;

CREATE TABLE control_audit_events (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id TEXT NOT NULL,
  action TEXT NOT NULL,
  target_type TEXT NOT NULL,
  target_id TEXT NOT NULL,
  request_id TEXT NOT NULL,
  details_json BLOB NOT NULL,
  created_at_ms INTEGER NOT NULL
) STRICT;

CREATE TRIGGER workers_active_insert_guard
BEFORE INSERT ON workers
WHEN NEW.active_deployment_id IS NOT NULL
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM worker_deployments
    WHERE id = NEW.active_deployment_id AND worker_id = NEW.id AND state = 'ready'
  ) THEN RAISE(ABORT, 'active deployment invariant') END;
END;

CREATE TRIGGER workers_active_update_guard
BEFORE UPDATE OF active_deployment_id ON workers
WHEN NEW.active_deployment_id IS NOT NULL
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM worker_deployments
    WHERE id = NEW.active_deployment_id AND worker_id = NEW.id AND state = 'ready'
  ) THEN RAISE(ABORT, 'active deployment invariant') END;
END;

CREATE TRIGGER deployment_transition_guard
BEFORE UPDATE OF state ON worker_deployments
WHEN OLD.state != NEW.state AND NOT (
  (OLD.state = 'staging' AND NEW.state IN ('validating', 'rejected')) OR
  (OLD.state = 'validating' AND NEW.state IN ('ready', 'rejected')) OR
  (OLD.state IN ('ready', 'rejected') AND NEW.state = 'deleting') OR
  (OLD.state = 'deleting' AND NEW.state = 'tombstoned')
)
BEGIN
  SELECT RAISE(ABORT, 'invalid deployment transition');
END;

CREATE TRIGGER deployment_immutable_guard
BEFORE UPDATE OF content_kind, artifact_sha256, artifact_size, artifact_schema_version,
  main_module, compatibility_date, compatibility_flags_json, limits_json,
  worker_code_sha256, loader_schema_version
ON worker_deployments
WHEN OLD.state != 'staging'
BEGIN
  SELECT RAISE(ABORT, 'immutable deployment');
END;

CREATE TRIGGER deployment_vars_insert_guard
BEFORE INSERT ON deployment_vars
WHEN (SELECT state FROM worker_deployments WHERE id = NEW.deployment_id) != 'staging'
BEGIN
  SELECT RAISE(ABORT, 'immutable deployment vars');
END;

CREATE TRIGGER deployment_vars_update_guard
BEFORE UPDATE ON deployment_vars
BEGIN
  SELECT RAISE(ABORT, 'immutable deployment vars');
END;

CREATE TRIGGER deployment_vars_delete_guard
BEFORE DELETE ON deployment_vars
WHEN (SELECT state FROM worker_deployments WHERE id = OLD.deployment_id) != 'deleting'
BEGIN
  SELECT RAISE(ABORT, 'immutable deployment vars');
END;

CREATE TRIGGER deployment_secrets_insert_guard
BEFORE INSERT ON deployment_secrets
WHEN (SELECT state FROM worker_deployments WHERE id = NEW.deployment_id) != 'staging'
BEGIN
  SELECT RAISE(ABORT, 'immutable deployment secrets');
END;

CREATE TRIGGER deployment_secrets_update_guard
BEFORE UPDATE ON deployment_secrets
BEGIN
  SELECT RAISE(ABORT, 'immutable deployment secrets');
END;

CREATE TRIGGER deployment_secrets_delete_guard
BEFORE DELETE ON deployment_secrets
WHEN (SELECT state FROM worker_deployments WHERE id = OLD.deployment_id) != 'deleting'
BEGIN
  SELECT RAISE(ABORT, 'immutable deployment secrets');
END;
