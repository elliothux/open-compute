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

CREATE TABLE worker_observability_settings (
  worker_id TEXT PRIMARY KEY REFERENCES workers(id) ON DELETE CASCADE,
  generation INTEGER NOT NULL CHECK(generation > 0),
  enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
  head_sampling_rate REAL CHECK(head_sampling_rate IS NULL OR
    (head_sampling_rate >= 0.0 AND head_sampling_rate <= 1.0)),
  logs_enabled INTEGER NOT NULL CHECK(logs_enabled IN (0, 1)),
  logs_head_sampling_rate REAL CHECK(logs_head_sampling_rate IS NULL OR
    (logs_head_sampling_rate >= 0.0 AND logs_head_sampling_rate <= 1.0)),
  invocation_logs INTEGER NOT NULL CHECK(invocation_logs IN (0, 1)),
  persist INTEGER NOT NULL CHECK(persist IN (0, 1)),
  updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE worker_versions (
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
  worker_code_sha256 BLOB NOT NULL CHECK(length(worker_code_sha256) = 32),
  loader_schema_version INTEGER NOT NULL,
  compatibility_date TEXT NOT NULL CHECK(length(compatibility_date) = 10),
  compatibility_flags_json BLOB NOT NULL CHECK(length(compatibility_flags_json) >= 2),
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

CREATE INDEX versions_worker_state
ON worker_versions(worker_id, state, version_number DESC);

CREATE TABLE version_annotations (
  version_id TEXT NOT NULL REFERENCES worker_versions(id),
  name TEXT NOT NULL,
  value TEXT NOT NULL,
  PRIMARY KEY(version_id, name),
  CHECK(name IN ('workers/message', 'workers/tag', 'workers/triggered_by')),
  CHECK(length(value) BETWEEN 1 AND 1000)
) WITHOUT ROWID, STRICT;

CREATE TRIGGER version_annotations_update_guard
BEFORE UPDATE ON version_annotations
BEGIN
  SELECT RAISE(ABORT, 'immutable version annotation');
END;

CREATE TRIGGER version_annotations_delete_guard
BEFORE DELETE ON version_annotations
WHEN (SELECT state FROM worker_versions WHERE id = OLD.version_id)
  NOT IN ('staging', 'deleting')
BEGIN
  SELECT RAISE(ABORT, 'immutable version annotation');
END;

CREATE TABLE worker_deployments (
  id TEXT PRIMARY KEY,
  worker_id TEXT NOT NULL REFERENCES workers(id),
  version_id TEXT NOT NULL REFERENCES worker_versions(id),
  source TEXT NOT NULL CHECK(source IN ('script_upload', 'versions_api', 'rollback', 'system')),
  annotations_json BLOB NOT NULL,
  created_at_ms INTEGER NOT NULL,
  deleted_at_ms INTEGER
) STRICT;

CREATE INDEX deployments_worker_created
ON worker_deployments(worker_id, created_at_ms DESC, id DESC);

CREATE TABLE version_vars (
  version_id TEXT NOT NULL REFERENCES worker_versions(id),
  name TEXT NOT NULL,
  value_json BLOB NOT NULL,
  PRIMARY KEY(version_id, name)
) WITHOUT ROWID, STRICT;

CREATE TABLE version_secrets (
  version_id TEXT NOT NULL REFERENCES worker_versions(id),
  name TEXT NOT NULL,
  revision_id TEXT NOT NULL,
  key_id TEXT NOT NULL,
  algorithm TEXT NOT NULL,
  nonce BLOB NOT NULL,
  ciphertext BLOB NOT NULL,
  PRIMARY KEY(version_id, name)
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
  version_id TEXT REFERENCES worker_versions(id),
  resource_id TEXT REFERENCES resources(id) DEFERRABLE INITIALLY DEFERRED,
  queue_id TEXT REFERENCES queues(id) DEFERRABLE INITIALLY DEFERRED,
  state TEXT NOT NULL CHECK(state IN ('running', 'complete', 'failed')),
  created_at_ms INTEGER NOT NULL,
  expires_at_ms INTEGER NOT NULL,
  PRIMARY KEY(account_id, scope, idempotency_key)
) WITHOUT ROWID, STRICT;

-- Every subsystem that keeps a version reachable registers a typed row
-- here. Deletion consults this registry instead of an incomplete COUNT spread
-- across product-specific tables.
CREATE TABLE version_referrers (
  version_id TEXT NOT NULL REFERENCES worker_versions(id),
  kind TEXT NOT NULL,
  ref_id TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  PRIMARY KEY(version_id, kind, ref_id)
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
    SELECT 1 FROM worker_deployments d JOIN worker_versions v ON v.id = d.version_id
    WHERE d.id = NEW.active_deployment_id AND d.worker_id = NEW.id
      AND d.deleted_at_ms IS NULL AND v.worker_id = NEW.id AND v.state = 'ready'
  ) THEN RAISE(ABORT, 'active deployment invariant') END;
END;

CREATE TRIGGER workers_active_update_guard
BEFORE UPDATE OF active_deployment_id ON workers
WHEN NEW.active_deployment_id IS NOT NULL
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM worker_deployments d JOIN worker_versions v ON v.id = d.version_id
    WHERE d.id = NEW.active_deployment_id AND d.worker_id = NEW.id
      AND d.deleted_at_ms IS NULL AND v.worker_id = NEW.id AND v.state = 'ready'
  ) THEN RAISE(ABORT, 'active deployment invariant') END;
END;

CREATE TRIGGER deployment_insert_guard
BEFORE INSERT ON worker_deployments
WHEN NOT EXISTS (
  SELECT 1 FROM worker_versions v
  WHERE v.id = NEW.version_id AND v.worker_id = NEW.worker_id AND v.state = 'ready'
)
BEGIN SELECT RAISE(ABORT, 'deployment target must be a ready version'); END;

CREATE TRIGGER deployment_immutable_guard
BEFORE UPDATE OF id,worker_id,version_id,source,created_at_ms ON worker_deployments
BEGIN SELECT RAISE(ABORT, 'deployment is immutable'); END;

CREATE TRIGGER deployment_delete_guard
BEFORE UPDATE OF deleted_at_ms ON worker_deployments
WHEN NEW.deleted_at_ms IS NOT NULL AND EXISTS (
  SELECT 1 FROM workers WHERE active_deployment_id = OLD.id
)
BEGIN SELECT RAISE(ABORT, 'active deployment cannot be deleted'); END;

CREATE TRIGGER version_transition_guard
BEFORE UPDATE OF state ON worker_versions
WHEN OLD.state != NEW.state AND NOT (
  (OLD.state = 'staging' AND NEW.state IN ('validating', 'rejected')) OR
  (OLD.state = 'validating' AND NEW.state IN ('ready', 'rejected')) OR
  (OLD.state IN ('ready', 'rejected') AND NEW.state = 'deleting') OR
  (OLD.state = 'deleting' AND NEW.state = 'tombstoned')
)
BEGIN
  SELECT RAISE(ABORT, 'invalid version transition');
END;

CREATE TRIGGER version_immutable_guard
BEFORE UPDATE OF content_kind, artifact_sha256, artifact_size, artifact_schema_version,
  main_module, worker_code_sha256, loader_schema_version, compatibility_date,
  compatibility_flags_json
ON worker_versions
WHEN OLD.state != 'staging'
BEGIN
  SELECT RAISE(ABORT, 'immutable version');
END;

CREATE TRIGGER version_vars_insert_guard
BEFORE INSERT ON version_vars
WHEN (SELECT state FROM worker_versions WHERE id = NEW.version_id) != 'staging'
BEGIN
  SELECT RAISE(ABORT, 'immutable version vars');
END;

CREATE TRIGGER version_vars_update_guard
BEFORE UPDATE ON version_vars
BEGIN
  SELECT RAISE(ABORT, 'immutable version vars');
END;

CREATE TRIGGER version_vars_delete_guard
BEFORE DELETE ON version_vars
WHEN (SELECT state FROM worker_versions WHERE id = OLD.version_id) != 'deleting'
BEGIN
  SELECT RAISE(ABORT, 'immutable version vars');
END;

CREATE TRIGGER version_secrets_insert_guard
BEFORE INSERT ON version_secrets
WHEN (SELECT state FROM worker_versions WHERE id = NEW.version_id) != 'staging'
BEGIN
  SELECT RAISE(ABORT, 'immutable version secrets');
END;

CREATE TRIGGER version_secrets_update_guard
BEFORE UPDATE ON version_secrets
BEGIN
  SELECT RAISE(ABORT, 'immutable version secrets');
END;

CREATE TRIGGER version_secrets_delete_guard
BEFORE DELETE ON version_secrets
WHEN (SELECT state FROM worker_versions WHERE id = OLD.version_id) != 'deleting'
BEGIN
  SELECT RAISE(ABORT, 'immutable version secrets');
END;
