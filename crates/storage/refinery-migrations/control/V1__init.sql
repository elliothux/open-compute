CREATE TABLE platform_meta (
  key TEXT NOT NULL PRIMARY KEY,
  value BLOB NOT NULL,
  updated_at_ms INTEGER NOT NULL
) STRICT;
CREATE TABLE accounts (
  id TEXT NOT NULL PRIMARY KEY,
  name TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  deleted_at_ms INTEGER
) STRICT;
CREATE UNIQUE INDEX accounts_live_name ON accounts(name) WHERE deleted_at_ms IS NULL;
CREATE TABLE workers (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  name TEXT NOT NULL,
  active_deployment_id TEXT REFERENCES worker_deployments(id) DEFERRABLE INITIALLY DEFERRED,
  do_storage_id TEXT NOT NULL,
  route_generation INTEGER NOT NULL DEFAULT 0 CHECK(route_generation >= 0),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  deleted_at_ms INTEGER, ownership TEXT NOT NULL DEFAULT 'tenant'
  CHECK(ownership IN ('tenant', 'system')),
  CHECK(length(name) BETWEEN 1 AND 63)
) STRICT;
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
CREATE TABLE resources (
  id TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  account_id TEXT NOT NULL REFERENCES accounts(id),
  kind TEXT NOT NULL CHECK(kind IN (
    'kv_namespace', 'r2_bucket', 'd1_database', 'do_namespace', 'vectorize_index',
    'ai_search_namespace', 'ai_search_instance'
  )),
  name TEXT NOT NULL,
  state TEXT NOT NULL CHECK(state IN (
    'creating', 'ready', 'deleting', 'tombstoned'
  )),
  availability TEXT NOT NULL DEFAULT 'healthy' CHECK(availability IN (
    'healthy', 'degraded', 'unavailable'
  )),
  availability_code TEXT,
  spec_generation INTEGER NOT NULL DEFAULT 1 CHECK(spec_generation >= 1),
  driver_schema_version INTEGER NOT NULL CHECK(driver_schema_version >= 1),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  deleted_at_ms INTEGER,
  CHECK(length(name) BETWEEN 1 AND 128),
  CHECK(availability_code IS NULL OR length(availability_code) BETWEEN 1 AND 128),
  CHECK((availability = 'healthy') = (availability_code IS NULL)),
  CHECK((state = 'tombstoned') = (deleted_at_ms IS NOT NULL))
) STRICT;
CREATE UNIQUE INDEX resources_live_name
ON resources(account_id, kind, name)
WHERE state != 'tombstoned';
CREATE INDEX resources_reconcile
ON resources(state, updated_at_ms, id)
WHERE state IN ('creating', 'deleting');
CREATE TABLE version_bindings (
  id TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  version_id TEXT NOT NULL REFERENCES worker_versions(id),
  name TEXT NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN (
    'kv_namespace', 'r2_bucket', 'd1_database', 'do_namespace', 'vectorize_index',
    'ai_search_namespace', 'ai_search_instance'
  )),
  resource_id TEXT NOT NULL REFERENCES resources(id),
  resource_spec_generation INTEGER NOT NULL CHECK(resource_spec_generation >= 1),
  capability_version INTEGER NOT NULL CHECK(capability_version >= 1),
  permissions_json BLOB NOT NULL,
  config_json BLOB NOT NULL,
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms INTEGER NOT NULL,
  UNIQUE(version_id, name),
  CHECK(length(name) BETWEEN 1 AND 64)
) STRICT;
CREATE INDEX version_bindings_resource
ON version_bindings(resource_id, version_id, id);
CREATE TABLE resource_referrers (
  resource_id TEXT NOT NULL REFERENCES resources(id),
  referrer_kind TEXT NOT NULL CHECK(referrer_kind IN (
    'version_binding', 'queue_dlq', 'queue_consumer',
    'workflow_definition', 'do_class', 'ai_search_instance'
  )),
  referrer_id TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  PRIMARY KEY(resource_id, referrer_kind, referrer_id)
) STRICT, WITHOUT ROWID;
CREATE TRIGGER resource_transition_guard
BEFORE UPDATE OF state ON resources
WHEN OLD.state != NEW.state AND NOT (
  (OLD.state = 'creating' AND NEW.state IN ('ready', 'deleting')) OR
  (OLD.state = 'ready' AND NEW.state = 'deleting') OR
  (OLD.state = 'deleting' AND NEW.state = 'tombstoned')
)
BEGIN
  SELECT RAISE(ABORT, 'invalid resource transition');
END;
CREATE TRIGGER resource_identity_immutable_guard
BEFORE UPDATE OF id, account_id, kind, driver_schema_version, created_at_ms ON resources
BEGIN
  SELECT RAISE(ABORT, 'immutable resource identity');
END;
CREATE TRIGGER resource_generation_guard
BEFORE UPDATE OF spec_generation ON resources
WHEN OLD.state != 'creating'
BEGIN
  SELECT RAISE(ABORT, 'immutable ready resource generation');
END;
CREATE TRIGGER resource_tombstone_guard
BEFORE UPDATE ON resources
WHEN OLD.state = 'tombstoned'
BEGIN
  SELECT RAISE(ABORT, 'immutable resource tombstone');
END;
CREATE TRIGGER resource_delete_referrer_guard
BEFORE UPDATE OF state ON resources
WHEN OLD.state != 'deleting' AND NEW.state = 'deleting'
  AND EXISTS (
    SELECT 1 FROM resource_referrers WHERE resource_id = OLD.id
  )
BEGIN
  SELECT RAISE(ABORT, 'resource is referenced');
END;
CREATE TRIGGER version_bindings_insert_guard
BEFORE INSERT ON version_bindings
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1
    FROM worker_versions d
    JOIN workers w ON w.id = d.worker_id
    JOIN resources r ON r.id = NEW.resource_id
    WHERE d.id = NEW.version_id
      AND d.state = 'staging'
      AND w.account_id = r.account_id
      AND r.state = 'ready'
      AND r.kind = NEW.kind
      AND r.spec_generation = NEW.resource_spec_generation
  ) THEN RAISE(ABORT, 'binding authority invariant') END;
  SELECT CASE WHEN NEW.name GLOB '*[^A-Za-z0-9_$]*'
    OR NEW.name GLOB '[^A-Za-z_$]*'
    OR NEW.name GLOB 'OPEN_COMPUTE_*'
    OR NEW.name GLOB '__*'
  THEN RAISE(ABORT, 'binding name invariant') END;
  SELECT CASE WHEN EXISTS (
    SELECT 1 FROM version_vars
    WHERE version_id = NEW.version_id AND name = NEW.name
  ) OR EXISTS (
    SELECT 1 FROM version_secrets
    WHERE version_id = NEW.version_id AND name = NEW.name
  ) THEN RAISE(ABORT, 'binding env name conflict') END;
END;
CREATE TRIGGER version_bindings_update_guard
BEFORE UPDATE ON version_bindings
BEGIN
  SELECT RAISE(ABORT, 'immutable version binding');
END;
CREATE TRIGGER version_bindings_delete_guard
BEFORE DELETE ON version_bindings
WHEN (SELECT state FROM worker_versions WHERE id = OLD.version_id)
  NOT IN ('staging', 'deleting')
BEGIN
  SELECT RAISE(ABORT, 'immutable version binding');
END;
CREATE TRIGGER version_bindings_referrer_insert
AFTER INSERT ON version_bindings
BEGIN
  INSERT INTO resource_referrers
    (resource_id, referrer_kind, referrer_id, created_at_ms)
  VALUES (NEW.resource_id, 'version_binding', NEW.id, NEW.created_at_ms);
END;
CREATE TRIGGER version_bindings_referrer_delete
AFTER DELETE ON version_bindings
BEGIN
  DELETE FROM resource_referrers
  WHERE resource_id = OLD.resource_id
    AND referrer_kind = 'version_binding'
    AND referrer_id = OLD.id;
END;
CREATE TRIGGER version_binding_referrer_insert_guard
BEFORE INSERT ON resource_referrers
WHEN NEW.referrer_kind = 'version_binding'
  AND NOT EXISTS (
    SELECT 1 FROM version_bindings
    WHERE id = NEW.referrer_id AND resource_id = NEW.resource_id
  )
BEGIN
  SELECT RAISE(ABORT, 'orphan version binding referrer');
END;
CREATE TRIGGER version_binding_referrer_delete_guard
BEFORE DELETE ON resource_referrers
WHEN OLD.referrer_kind = 'version_binding'
  AND EXISTS (
    SELECT 1
    FROM version_bindings b
    JOIN worker_versions d ON d.id = b.version_id
    JOIN workers w ON w.id = d.worker_id
    WHERE b.id = OLD.referrer_id
      AND b.resource_id = OLD.resource_id
      AND w.deleted_at_ms IS NULL
  )
BEGIN
  SELECT RAISE(ABORT, 'live version binding referrer');
END;
CREATE TABLE kv_namespaces (
  resource_id          TEXT PRIMARY KEY REFERENCES resources(id),
  storage_key          TEXT NOT NULL UNIQUE,
  schema_version       INTEGER NOT NULL CHECK(schema_version >= 1),
  quota_bytes          INTEGER NOT NULL CHECK(quota_bytes >= 268435456),
  created_at_ms        INTEGER NOT NULL,
  last_opened_at_ms    INTEGER,
  last_quick_check_ms  INTEGER,
  last_backup_at_ms    INTEGER,
  restore_backup_id    TEXT REFERENCES kv_backups(id)
) STRICT;
CREATE TABLE kv_backups (
  id                    TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  source_resource_id    TEXT NOT NULL REFERENCES resources(id),
  state                 TEXT NOT NULL CHECK(state IN (
                          'creating', 'ready', 'failed', 'deleting', 'tombstoned'
                        )),
  object_key            TEXT,
  sha256                BLOB CHECK(sha256 IS NULL OR length(sha256) = 32),
  size_bytes            INTEGER CHECK(size_bytes IS NULL OR size_bytes >= 0),
  kv_schema_version     INTEGER NOT NULL CHECK(kv_schema_version >= 1),
  created_at_ms         INTEGER NOT NULL,
  completed_at_ms       INTEGER,
  error_code            TEXT,
  idempotency_key       TEXT NOT NULL CHECK(length(idempotency_key) BETWEEN 1 AND 128),
  request_fingerprint   BLOB NOT NULL CHECK(length(request_fingerprint) = 32),
  UNIQUE(source_resource_id, idempotency_key),
  CHECK((state = 'ready') =
        (object_key IS NOT NULL AND sha256 IS NOT NULL AND size_bytes IS NOT NULL))
) STRICT;
CREATE INDEX kv_backups_source
ON kv_backups(source_resource_id, created_at_ms, id);
CREATE TRIGGER kv_namespace_insert_guard
BEFORE INSERT ON kv_namespaces
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM resources
    WHERE id = NEW.resource_id
      AND kind = 'kv_namespace'
      AND state = 'creating'
      AND created_at_ms = NEW.created_at_ms
  ) THEN RAISE(ABORT, 'kv namespace authority invariant') END;
END;
CREATE TRIGGER kv_namespace_identity_immutable_guard
BEFORE UPDATE OF resource_id, storage_key, schema_version, quota_bytes, created_at_ms,
                 restore_backup_id
ON kv_namespaces
BEGIN
  SELECT RAISE(ABORT, 'immutable kv namespace identity');
END;
CREATE TRIGGER kv_namespace_delete_guard
BEFORE DELETE ON kv_namespaces
WHEN (SELECT state FROM resources WHERE id = OLD.resource_id)
  NOT IN ('deleting', 'tombstoned')
BEGIN
  SELECT RAISE(ABORT, 'live kv namespace locator');
END;
CREATE TRIGGER kv_resource_tombstone_retire_locator
AFTER UPDATE OF state ON resources
WHEN NEW.state = 'tombstoned' AND NEW.kind = 'kv_namespace'
BEGIN
  DELETE FROM kv_namespaces WHERE resource_id = NEW.id;
END;
CREATE TRIGGER kv_backup_insert_guard
BEFORE INSERT ON kv_backups
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM resources
    WHERE id = NEW.source_resource_id AND kind = 'kv_namespace'
  ) THEN RAISE(ABORT, 'kv backup source invariant') END;
END;
CREATE TRIGGER kv_backup_identity_immutable_guard
BEFORE UPDATE OF id, source_resource_id, kv_schema_version, created_at_ms,
                 idempotency_key, request_fingerprint
ON kv_backups
BEGIN
  SELECT RAISE(ABORT, 'immutable kv backup identity');
END;
CREATE TABLE r2_buckets (
  resource_id           TEXT PRIMARY KEY REFERENCES resources(id),
  physical_prefix       TEXT NOT NULL UNIQUE,
  schema_version        INTEGER NOT NULL CHECK(schema_version >= 1),
  max_object_bytes      INTEGER NOT NULL CHECK(max_object_bytes > 0),
  object_authority_sha256 BLOB NOT NULL CHECK(length(object_authority_sha256) = 32),
  created_at_ms         INTEGER NOT NULL,
  delete_started_at_ms  INTEGER,
  last_probe_at_ms      INTEGER
) STRICT;
CREATE TRIGGER r2_bucket_insert_guard
BEFORE INSERT ON r2_buckets
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM resources
    WHERE id = NEW.resource_id
      AND kind = 'r2_bucket'
      AND state = 'creating'
      AND created_at_ms = NEW.created_at_ms
  ) THEN RAISE(ABORT, 'r2 bucket authority invariant') END;
END;
CREATE TRIGGER r2_bucket_identity_immutable_guard
BEFORE UPDATE OF resource_id, physical_prefix, schema_version,
                 max_object_bytes, object_authority_sha256, created_at_ms
ON r2_buckets
BEGIN
  SELECT RAISE(ABORT, 'immutable r2 bucket identity');
END;
CREATE TRIGGER r2_bucket_delete_guard
BEFORE DELETE ON r2_buckets
WHEN (SELECT state FROM resources WHERE id = OLD.resource_id) != 'tombstoned'
BEGIN
  SELECT RAISE(ABORT, 'live r2 bucket locator');
END;
CREATE TRIGGER r2_resource_tombstone_guard
BEFORE UPDATE OF state ON resources
WHEN NEW.state = 'tombstoned' AND NEW.kind = 'r2_bucket'
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM r2_buckets
    WHERE resource_id = NEW.id AND delete_started_at_ms IS NOT NULL
  ) THEN RAISE(ABORT, 'r2 bucket deletion not finalized') END;
END;
CREATE TRIGGER r2_resource_tombstone_retire_locator
AFTER UPDATE OF state ON resources
WHEN NEW.state = 'tombstoned' AND NEW.kind = 'r2_bucket'
BEGIN
  DELETE FROM r2_buckets WHERE resource_id = NEW.id;
END;
CREATE TABLE r2_objects (
  resource_id TEXT NOT NULL REFERENCES r2_buckets(resource_id) ON DELETE CASCADE,
  object_key TEXT NOT NULL,
  account_id TEXT NOT NULL,
  object_version TEXT NOT NULL,
  ssec_key_md5 TEXT,
  ssec_envelope TEXT,
  updated_at_ms INTEGER NOT NULL,
  PRIMARY KEY (resource_id, object_key),
  CHECK((ssec_key_md5 IS NULL) = (ssec_envelope IS NULL)),
  CHECK(ssec_key_md5 IS NULL OR (
    length(ssec_key_md5) = 32 AND ssec_key_md5 NOT GLOB '*[^0-9a-f]*'
  ))
) STRICT;
CREATE TABLE r2_object_mutations (
  resource_id TEXT NOT NULL REFERENCES r2_buckets(resource_id) ON DELETE CASCADE,
  object_key TEXT NOT NULL,
  account_id TEXT NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN ('put', 'delete')),
  pending_version TEXT,
  pending_ssec_key_md5 TEXT,
  pending_ssec_envelope TEXT,
  started_at_ms INTEGER NOT NULL,
  PRIMARY KEY (resource_id, object_key),
  CHECK((pending_ssec_key_md5 IS NULL) = (pending_ssec_envelope IS NULL)),
  CHECK(pending_ssec_key_md5 IS NULL OR (
    length(pending_ssec_key_md5) = 32 AND pending_ssec_key_md5 NOT GLOB '*[^0-9a-f]*'
  )),
  CHECK((kind = 'put') = (pending_version IS NOT NULL)),
  CHECK(kind = 'put' OR (pending_ssec_key_md5 IS NULL AND pending_ssec_envelope IS NULL))
) STRICT;
CREATE TABLE r2_multipart_uploads (
  upload_id TEXT PRIMARY KEY,
  resource_id TEXT NOT NULL REFERENCES r2_buckets(resource_id) ON DELETE CASCADE,
  account_id TEXT NOT NULL,
  object_key TEXT NOT NULL,
  provider_upload_id TEXT,
  storage_class TEXT NOT NULL CHECK(storage_class IN ('Standard', 'InfrequentAccess')),
  http_metadata TEXT NOT NULL,
  custom_metadata TEXT NOT NULL,
  ssec_key_md5 TEXT,
  ssec_envelope TEXT,
  object_version TEXT NOT NULL,
  completion_manifest TEXT,
  completed_metadata TEXT,
  state TEXT NOT NULL CHECK(state IN ('initiating', 'create_unknown', 'open', 'completing', 'completed', 'aborting', 'aborted')),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  CHECK((ssec_key_md5 IS NULL) = (ssec_envelope IS NULL)),
  CHECK(ssec_key_md5 IS NULL OR (
    length(ssec_key_md5) = 32 AND ssec_key_md5 NOT GLOB '*[^0-9a-f]*'
  )),
  CHECK(state IN ('initiating', 'create_unknown') OR provider_upload_id IS NOT NULL),
  CHECK((state IN ('completing', 'completed')) = (completion_manifest IS NOT NULL)),
  CHECK((state = 'completed') = (completed_metadata IS NOT NULL))
) STRICT;
CREATE INDEX r2_multipart_uploads_resource_state
  ON r2_multipart_uploads(resource_id, state);
CREATE TABLE r2_multipart_parts (
  upload_id TEXT NOT NULL REFERENCES r2_multipart_uploads(upload_id) ON DELETE CASCADE,
  part_number INTEGER NOT NULL CHECK(part_number >= 1 AND part_number <= 10000),
  etag TEXT NOT NULL,
  size INTEGER NOT NULL CHECK(size >= 0),
  uploaded_at_ms INTEGER NOT NULL,
  PRIMARY KEY (upload_id, part_number)
) STRICT;
CREATE TABLE d1_backups (
  id                    TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  source_resource_id    TEXT NOT NULL REFERENCES resources(id),
  state                 TEXT NOT NULL CHECK(state IN (
                          'creating', 'ready', 'failed', 'deleting', 'tombstoned'
                        )),
  object_key            TEXT,
  sha256                BLOB CHECK(sha256 IS NULL OR length(sha256) = 32),
  size_bytes            INTEGER CHECK(size_bytes IS NULL OR size_bytes >= 0),
  d1_schema_version     INTEGER NOT NULL CHECK(d1_schema_version >= 1),
  sqlite_user_version   INTEGER NOT NULL CHECK(sqlite_user_version >= 0),
  created_at_ms         INTEGER NOT NULL,
  completed_at_ms       INTEGER,
  error_code            TEXT,
  idempotency_key       TEXT NOT NULL CHECK(length(idempotency_key) BETWEEN 1 AND 128),
  request_fingerprint   BLOB NOT NULL CHECK(length(request_fingerprint) = 32),
  UNIQUE(source_resource_id, idempotency_key),
  CHECK((state = 'ready') =
        (object_key IS NOT NULL AND sha256 IS NOT NULL AND size_bytes IS NOT NULL))
) STRICT;
CREATE INDEX d1_backups_source
ON d1_backups(source_resource_id, created_at_ms, id);
CREATE TABLE d1_databases (
  resource_id          TEXT PRIMARY KEY REFERENCES resources(id),
  storage_key          TEXT NOT NULL UNIQUE,
  schema_version       INTEGER NOT NULL CHECK(schema_version >= 1),
  quota_bytes          INTEGER NOT NULL CHECK(quota_bytes >= 67108864),
  created_at_ms        INTEGER NOT NULL,
  last_opened_at_ms    INTEGER,
  last_quick_check_ms  INTEGER,
  last_backup_at_ms    INTEGER,
  restore_backup_id    TEXT REFERENCES d1_backups(id)
) STRICT;
CREATE TRIGGER d1_database_insert_guard
BEFORE INSERT ON d1_databases
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM resources
    WHERE id = NEW.resource_id
      AND kind = 'd1_database'
      AND state = 'creating'
      AND created_at_ms = NEW.created_at_ms
  ) THEN RAISE(ABORT, 'd1 database authority invariant') END;
END;
CREATE TRIGGER d1_database_identity_immutable_guard
BEFORE UPDATE OF resource_id, storage_key, schema_version, quota_bytes, created_at_ms,
                 restore_backup_id
ON d1_databases
BEGIN
  SELECT RAISE(ABORT, 'immutable d1 database identity');
END;
CREATE TRIGGER d1_database_delete_guard
BEFORE DELETE ON d1_databases
WHEN (SELECT state FROM resources WHERE id = OLD.resource_id)
  NOT IN ('deleting', 'tombstoned')
BEGIN
  SELECT RAISE(ABORT, 'live d1 database locator');
END;
CREATE TRIGGER d1_resource_tombstone_retire_locator
AFTER UPDATE OF state ON resources
WHEN NEW.state = 'tombstoned' AND NEW.kind = 'd1_database'
BEGIN
  DELETE FROM d1_databases WHERE resource_id = NEW.id;
END;
CREATE TRIGGER d1_backup_insert_guard
BEFORE INSERT ON d1_backups
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM resources
    WHERE id = NEW.source_resource_id AND kind = 'd1_database'
  ) THEN RAISE(ABORT, 'd1 backup source invariant') END;
END;
CREATE TRIGGER d1_backup_identity_immutable_guard
BEFORE UPDATE OF id, source_resource_id, d1_schema_version, sqlite_user_version,
                 created_at_ms, idempotency_key, request_fingerprint
ON d1_backups
BEGIN
  SELECT RAISE(ABORT, 'immutable d1 backup identity');
END;
CREATE TABLE d1_snapshots (
  resource_id          TEXT NOT NULL REFERENCES d1_databases(resource_id) ON DELETE CASCADE,
  session_version      INTEGER NOT NULL CHECK(session_version >= 0),
  snapshot_key         TEXT NOT NULL UNIQUE CHECK(
                         length(snapshot_key) BETWEEN 1 AND 512
                         AND instr(snapshot_key, '..') = 0
                       ),
  sha256               BLOB NOT NULL CHECK(length(sha256) = 32),
  size_bytes           INTEGER NOT NULL CHECK(size_bytes > 0),
  created_at_ms        INTEGER NOT NULL CHECK(created_at_ms >= 0),
  PRIMARY KEY(resource_id, session_version)
) STRICT, WITHOUT ROWID;
CREATE INDEX d1_snapshots_timestamp
ON d1_snapshots(resource_id, created_at_ms DESC, session_version DESC);
CREATE TRIGGER d1_snapshot_immutable_guard
BEFORE UPDATE ON d1_snapshots
BEGIN
  SELECT RAISE(ABORT, 'immutable completed d1 snapshot');
END;
CREATE TABLE d1_transfer_sessions (
  id                    TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  resource_id           TEXT NOT NULL,
  kind                  TEXT NOT NULL CHECK(kind IN ('export', 'import')),
  state                 TEXT NOT NULL CHECK(state IN (
                          'preparing', 'uploading', 'uploaded', 'ingesting',
                          'complete', 'failed', 'expired'
                        )),
  at_session_version    INTEGER NOT NULL CHECK(at_session_version >= 0),
  result_session_version INTEGER CHECK(result_session_version >= 0),
  filename              TEXT NOT NULL CHECK(
                          length(filename) BETWEEN 1 AND 255
                          AND instr(filename, '/') = 0
                          AND instr(filename, char(0)) = 0
                          AND filename NOT IN ('.', '..')
                        ),
  file_key              TEXT CHECK(
                          file_key IS NULL OR (
                            length(file_key) BETWEEN 1 AND 512
                            AND instr(file_key, '..') = 0
                          )
                        ),
  etag_md5              BLOB CHECK(etag_md5 IS NULL OR length(etag_md5) = 16),
  sha256                BLOB CHECK(sha256 IS NULL OR length(sha256) = 32),
  size_bytes            INTEGER CHECK(size_bytes IS NULL OR size_bytes > 0),
  token_fingerprint     BLOB NOT NULL CHECK(length(token_fingerprint) = 32),
  token_action          TEXT NOT NULL CHECK(token_action IN ('upload', 'download')),
  token_expires_at_ms   INTEGER NOT NULL,
  num_queries           INTEGER CHECK(num_queries IS NULL OR num_queries >= 0),
  duration_ms           REAL CHECK(duration_ms IS NULL OR duration_ms >= 0),
  rows_read             INTEGER CHECK(rows_read IS NULL OR rows_read >= 0),
  rows_written          INTEGER CHECK(rows_written IS NULL OR rows_written >= 0),
  result_size_after     INTEGER CHECK(result_size_after IS NULL OR result_size_after >= 0),
  created_at_ms         INTEGER NOT NULL CHECK(created_at_ms >= 0),
  updated_at_ms         INTEGER NOT NULL CHECK(updated_at_ms >= created_at_ms),
  completed_at_ms       INTEGER,
  error_code            TEXT,
  FOREIGN KEY(resource_id)
    REFERENCES d1_databases(resource_id) ON DELETE CASCADE,
  FOREIGN KEY(resource_id, at_session_version)
    REFERENCES d1_snapshots(resource_id, session_version),
  FOREIGN KEY(resource_id, result_session_version)
    REFERENCES d1_snapshots(resource_id, session_version),
  UNIQUE(resource_id, kind, filename),
  CHECK(token_expires_at_ms > created_at_ms),
  CHECK((kind = 'export' AND token_action = 'download' AND etag_md5 IS NULL)
     OR (kind = 'import' AND token_action = 'upload' AND etag_md5 IS NOT NULL)),
  CHECK((file_key IS NULL AND sha256 IS NULL AND size_bytes IS NULL)
     OR (file_key IS NOT NULL AND sha256 IS NOT NULL AND size_bytes IS NOT NULL)),
  CHECK(state NOT IN ('preparing', 'uploading')
        OR (file_key IS NULL AND sha256 IS NULL AND size_bytes IS NULL)),
  CHECK(state NOT IN ('uploaded', 'ingesting', 'complete')
        OR (file_key IS NOT NULL AND sha256 IS NOT NULL AND size_bytes IS NOT NULL)),
  CHECK(kind != 'export' OR (num_queries IS NULL AND duration_ms IS NULL AND rows_read IS NULL
                             AND rows_written IS NULL AND result_size_after IS NULL)),
  CHECK(state NOT IN ('preparing', 'uploading', 'uploaded')
        OR (num_queries IS NULL AND duration_ms IS NULL
            AND rows_written IS NULL AND result_size_after IS NULL)),
  CHECK(kind != 'import' OR state NOT IN ('ingesting', 'complete')
        OR (num_queries IS NOT NULL AND duration_ms IS NOT NULL AND rows_read IS NOT NULL
            AND rows_written IS NOT NULL AND result_size_after IS NOT NULL)),
  CHECK((state = 'complete' AND kind = 'import') =
        (result_session_version IS NOT NULL AND num_queries IS NOT NULL)),
  CHECK((state IN ('complete', 'failed', 'expired')) = (completed_at_ms IS NOT NULL)),
  CHECK((state = 'failed') = (error_code IS NOT NULL))
) STRICT;
CREATE INDEX d1_transfer_sessions_resource
ON d1_transfer_sessions(resource_id, created_at_ms DESC, id);
CREATE UNIQUE INDEX d1_transfer_sessions_active
ON d1_transfer_sessions(resource_id)
WHERE state IN ('preparing', 'uploading', 'uploaded', 'ingesting');
CREATE TRIGGER d1_transfer_identity_immutable_guard
BEFORE UPDATE OF id, resource_id, kind, at_session_version, filename, etag_md5,
                 token_fingerprint, token_action, token_expires_at_ms, created_at_ms
ON d1_transfer_sessions
BEGIN
  SELECT RAISE(ABORT, 'immutable d1 transfer identity');
END;
CREATE TRIGGER d1_transfer_transition_guard
BEFORE UPDATE OF state ON d1_transfer_sessions
BEGIN
  SELECT CASE WHEN NOT (
       (OLD.state = 'preparing' AND NEW.state IN ('complete', 'failed', 'expired'))
    OR (OLD.state = 'uploading' AND NEW.state IN ('uploaded', 'failed', 'expired'))
    OR (OLD.state = 'uploaded' AND NEW.state IN ('ingesting', 'failed', 'expired'))
    OR (OLD.state = 'ingesting' AND NEW.state IN ('complete', 'failed', 'expired'))
  ) THEN RAISE(ABORT, 'invalid d1 transfer transition') END;
  SELECT CASE WHEN NEW.updated_at_ms < OLD.updated_at_ms
    THEN RAISE(ABORT, 'd1 transfer time moved backwards') END;
END;
CREATE TRIGGER d1_transfer_file_evidence_immutable_guard
BEFORE UPDATE OF file_key, sha256, size_bytes ON d1_transfer_sessions
WHEN OLD.file_key IS NOT NULL AND (
     NEW.file_key IS NOT OLD.file_key
  OR NEW.sha256 IS NOT OLD.sha256
  OR NEW.size_bytes IS NOT OLD.size_bytes
)
BEGIN
  SELECT RAISE(ABORT, 'immutable d1 transfer file evidence');
END;
CREATE TRIGGER d1_transfer_result_guard
BEFORE UPDATE OF result_session_version, num_queries, duration_ms, rows_read, rows_written,
                 result_size_after
ON d1_transfer_sessions
WHEN NOT (
  (OLD.state = 'uploaded' AND NEW.state = 'ingesting'
   AND OLD.result_session_version IS NULL AND NEW.result_session_version IS NULL
   AND OLD.num_queries IS NULL AND NEW.num_queries IS NOT NULL
   AND OLD.duration_ms IS NULL AND NEW.duration_ms IS NOT NULL
   AND OLD.rows_read IS NULL AND NEW.rows_read IS NOT NULL
   AND OLD.rows_written IS NULL AND NEW.rows_written IS NOT NULL
   AND OLD.result_size_after IS NULL AND NEW.result_size_after IS NOT NULL)
  OR
  (OLD.state = 'ingesting' AND NEW.state = 'complete'
   AND OLD.result_session_version IS NULL AND NEW.result_session_version IS NOT NULL
   AND OLD.num_queries IS NEW.num_queries
   AND OLD.duration_ms IS NEW.duration_ms
   AND OLD.rows_read IS NEW.rows_read
   AND OLD.rows_written IS NEW.rows_written
   AND OLD.result_size_after IS NEW.result_size_after)
)
BEGIN
  SELECT RAISE(ABORT, 'invalid d1 transfer result');
END;
CREATE TRIGGER d1_transfer_terminal_immutable_guard
BEFORE UPDATE ON d1_transfer_sessions
WHEN OLD.state IN ('complete', 'failed', 'expired')
BEGIN
  SELECT RAISE(ABORT, 'immutable terminal d1 transfer');
END;
CREATE TABLE d1_restore_intents (
  id                    TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  resource_id           TEXT NOT NULL UNIQUE,
  source_session_version INTEGER NOT NULL CHECK(source_session_version >= 0),
  previous_session_version INTEGER NOT NULL CHECK(previous_session_version >= 0),
  result_session_version INTEGER NOT NULL CHECK(
                           result_session_version = previous_session_version + 1
                         ),
  request_fingerprint   BLOB NOT NULL CHECK(length(request_fingerprint) = 32),
  created_at_ms         INTEGER NOT NULL CHECK(created_at_ms >= 0),
  FOREIGN KEY(resource_id)
    REFERENCES d1_databases(resource_id) ON DELETE CASCADE,
  FOREIGN KEY(resource_id, source_session_version)
    REFERENCES d1_snapshots(resource_id, session_version),
  FOREIGN KEY(resource_id, previous_session_version)
    REFERENCES d1_snapshots(resource_id, session_version)
) STRICT;
CREATE TRIGGER d1_restore_intent_immutable_guard
BEFORE UPDATE ON d1_restore_intents
BEGIN
  SELECT RAISE(ABORT, 'immutable d1 restore intent');
END;
CREATE TABLE do_namespaces (
  resource_id           TEXT PRIMARY KEY REFERENCES resources(id),
  owner_worker_id       TEXT NOT NULL REFERENCES workers(id),
  class_name            TEXT NOT NULL,
  do_storage_id         TEXT NOT NULL,
  namespace_storage_key TEXT NOT NULL UNIQUE,
  schema_version        INTEGER NOT NULL CHECK(schema_version >= 1),
  lifecycle_state       TEXT NOT NULL DEFAULT 'active' CHECK(lifecycle_state IN (
                          'pending', 'active', 'retired'
                        )),
  migration_tag         TEXT,
  previous_class_name   TEXT,
  created_at_ms         INTEGER NOT NULL,
  CHECK(length(class_name) BETWEEN 1 AND 128),
  CHECK(class_name NOT GLOB '*[^A-Za-z0-9_$]*'),
  CHECK(class_name NOT GLOB '[^A-Za-z_$]*'),
  CHECK(length(do_storage_id) BETWEEN 1 AND 128),
  CHECK(migration_tag IS NULL OR length(migration_tag) BETWEEN 1 AND 128),
  CHECK(previous_class_name IS NULL OR (
    length(previous_class_name) BETWEEN 1 AND 128 AND
    previous_class_name NOT GLOB '*[^A-Za-z0-9_$]*' AND
    previous_class_name NOT GLOB '[^A-Za-z_$]*'
  )),
  CHECK((lifecycle_state = 'pending') = (migration_tag IS NOT NULL)),
  CHECK(length(namespace_storage_key) = 64 AND namespace_storage_key = lower(namespace_storage_key)),
  UNIQUE(owner_worker_id, class_name)
) STRICT;
CREATE TABLE do_objects (
  namespace_resource_id TEXT NOT NULL REFERENCES do_namespaces(resource_id) ON DELETE CASCADE,
  object_id             TEXT NOT NULL,
  generation            INTEGER NOT NULL CHECK(generation >= 1),
  state                 TEXT NOT NULL CHECK(state IN (
                          'creating', 'ready', 'deleting', 'tombstoned'
                        )),
  created_at_ms         INTEGER NOT NULL,
  updated_at_ms         INTEGER NOT NULL,
  deleted_at_ms         INTEGER,
  PRIMARY KEY(namespace_resource_id, object_id, generation),
  CHECK(length(object_id) = 64 AND object_id = lower(object_id)),
  CHECK(object_id NOT GLOB '*[^0-9a-f]*'),
  CHECK((state = 'tombstoned') = (deleted_at_ms IS NOT NULL))
) STRICT;
CREATE UNIQUE INDEX do_objects_live_identity
ON do_objects(namespace_resource_id, object_id)
WHERE state != 'tombstoned';
CREATE INDEX do_objects_reconcile
ON do_objects(state, updated_at_ms, namespace_resource_id, object_id)
WHERE state IN ('creating', 'deleting');
CREATE TRIGGER do_namespace_insert_guard
BEFORE INSERT ON do_namespaces
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1
    FROM resources r
    JOIN workers w ON w.id = NEW.owner_worker_id
    WHERE r.id = NEW.resource_id
      AND r.kind = 'do_namespace'
      AND r.state = 'creating'
      AND r.account_id = w.account_id
      AND w.deleted_at_ms IS NULL
      AND w.do_storage_id = NEW.do_storage_id
      AND r.created_at_ms = NEW.created_at_ms
  ) THEN RAISE(ABORT, 'durable object namespace authority invariant') END;
END;
CREATE TRIGGER do_namespace_identity_immutable_guard
BEFORE UPDATE OF resource_id, owner_worker_id, do_storage_id,
  namespace_storage_key, schema_version, created_at_ms ON do_namespaces
BEGIN
  SELECT RAISE(ABORT, 'immutable durable object namespace identity');
END;
CREATE TRIGGER do_namespace_lifecycle_guard
BEFORE UPDATE OF lifecycle_state ON do_namespaces
WHEN OLD.lifecycle_state != NEW.lifecycle_state AND NOT (
  (OLD.lifecycle_state = 'pending' AND NEW.lifecycle_state IN ('active', 'retired')) OR
  (OLD.lifecycle_state = 'active' AND NEW.lifecycle_state IN ('pending', 'retired')) OR
  (OLD.lifecycle_state = 'retired' AND NEW.lifecycle_state = 'pending')
)
BEGIN
  SELECT RAISE(ABORT, 'invalid durable object namespace lifecycle transition');
END;
CREATE TRIGGER do_namespace_migration_metadata_guard
BEFORE UPDATE OF class_name, migration_tag, previous_class_name ON do_namespaces
WHEN NOT (
  (OLD.lifecycle_state = 'active' AND NEW.lifecycle_state = 'pending' AND
   NEW.class_name != OLD.class_name AND NEW.migration_tag IS NOT NULL AND
   NEW.previous_class_name = OLD.class_name) OR
  (OLD.lifecycle_state = 'retired' AND NEW.lifecycle_state = 'pending' AND
   NEW.class_name = OLD.class_name AND NEW.migration_tag IS NOT NULL AND
   NEW.previous_class_name = OLD.class_name) OR
  (OLD.lifecycle_state = 'pending' AND NEW.lifecycle_state = 'active' AND
   (NEW.class_name = OLD.class_name OR NEW.class_name = OLD.previous_class_name) AND
   NEW.migration_tag IS NULL AND NEW.previous_class_name IS NULL) OR
  (OLD.lifecycle_state = 'pending' AND NEW.lifecycle_state = 'retired' AND
   NEW.class_name = OLD.class_name AND NEW.migration_tag IS NULL AND
   NEW.previous_class_name IS NULL)
)
BEGIN
  SELECT RAISE(ABORT, 'invalid durable object namespace migration metadata');
END;
CREATE TABLE worker_do_migrations (
  worker_id      TEXT NOT NULL REFERENCES workers(id),
  tag            TEXT NOT NULL,
  old_tag        TEXT,
  plan_sha256    BLOB NOT NULL CHECK(length(plan_sha256) = 32),
  version_id     TEXT NOT NULL REFERENCES worker_versions(id),
  created_at_ms  INTEGER NOT NULL,
  PRIMARY KEY(worker_id, tag),
  UNIQUE(worker_id, version_id),
  FOREIGN KEY(worker_id, old_tag)
    REFERENCES worker_do_migrations(worker_id, tag),
  CHECK(length(tag) BETWEEN 1 AND 128),
  CHECK(old_tag IS NULL OR length(old_tag) BETWEEN 1 AND 128)
) STRICT;
CREATE TABLE worker_do_migration_heads (
  worker_id      TEXT PRIMARY KEY REFERENCES workers(id),
  current_tag    TEXT NOT NULL,
  updated_at_ms  INTEGER NOT NULL,
  FOREIGN KEY(worker_id, current_tag)
    REFERENCES worker_do_migrations(worker_id, tag),
  CHECK(length(current_tag) BETWEEN 1 AND 128)
) STRICT;
CREATE TRIGGER worker_do_migration_insert_guard
BEFORE INSERT ON worker_do_migrations
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM worker_versions v
    WHERE v.id = NEW.version_id
      AND v.worker_id = NEW.worker_id
      AND v.state = 'ready'
      AND v.deleted_at_ms IS NULL
  ) THEN RAISE(ABORT, 'durable object migration version authority invariant') END;
END;
CREATE TRIGGER worker_do_migration_update_guard
BEFORE UPDATE ON worker_do_migrations
BEGIN
  SELECT RAISE(ABORT, 'immutable durable object migration authority');
END;
CREATE TRIGGER worker_do_migration_delete_guard
BEFORE DELETE ON worker_do_migrations
BEGIN
  SELECT RAISE(ABORT, 'immutable durable object migration authority');
END;
CREATE TRIGGER do_namespace_delete_guard
BEFORE DELETE ON do_namespaces
WHEN (SELECT state FROM resources WHERE id = OLD.resource_id) != 'tombstoned'
BEGIN
  SELECT RAISE(ABORT, 'live durable object namespace locator');
END;
CREATE TRIGGER do_object_insert_guard
BEFORE INSERT ON do_objects
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM resources r
    WHERE r.id = NEW.namespace_resource_id
      AND r.kind = 'do_namespace'
      AND r.state IN ('ready', 'deleting')
  ) THEN RAISE(ABORT, 'durable object registry authority invariant') END;
  SELECT CASE WHEN EXISTS (
    SELECT 1 FROM do_objects old
    WHERE old.namespace_resource_id = NEW.namespace_resource_id
      AND old.object_id = NEW.object_id
      AND old.generation >= NEW.generation
  ) THEN RAISE(ABORT, 'durable object generation invariant') END;
END;
CREATE TRIGGER do_object_identity_immutable_guard
BEFORE UPDATE OF namespace_resource_id, object_id, generation, created_at_ms ON do_objects
BEGIN
  SELECT RAISE(ABORT, 'immutable durable object identity');
END;
CREATE TRIGGER do_object_transition_guard
BEFORE UPDATE OF state ON do_objects
WHEN OLD.state != NEW.state AND NOT (
  (OLD.state = 'creating' AND NEW.state IN ('ready', 'deleting')) OR
  (OLD.state = 'ready' AND NEW.state = 'deleting') OR
  (OLD.state = 'deleting' AND NEW.state = 'tombstoned')
)
BEGIN
  SELECT RAISE(ABORT, 'invalid durable object transition');
END;
CREATE TRIGGER do_object_tombstone_guard
BEFORE UPDATE ON do_objects
WHEN OLD.state = 'tombstoned'
BEGIN
  SELECT RAISE(ABORT, 'immutable durable object tombstone');
END;
CREATE TRIGGER do_resource_tombstone_retire_locator
AFTER UPDATE OF state ON resources
WHEN NEW.state = 'tombstoned' AND NEW.kind = 'do_namespace'
BEGIN
  DELETE FROM do_namespaces WHERE resource_id = NEW.id;
END;
CREATE TABLE queues (
  id                       TEXT PRIMARY KEY
                           CHECK(length(id) = 36 AND id = lower(id)),
  account_id               TEXT NOT NULL REFERENCES accounts(id),
  name                     TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 128),
  state                    TEXT NOT NULL CHECK(state IN (
                             'creating', 'ready', 'deleting', 'tombstoned'
                           )),
  availability             TEXT NOT NULL CHECK(availability IN (
                             'healthy', 'degraded', 'unavailable'
                           )),
  availability_code        TEXT,
  lifecycle_generation     INTEGER NOT NULL CHECK(lifecycle_generation >= 1),
  config_generation        INTEGER NOT NULL CHECK(config_generation >= 1),
  delivery_delay_seconds   INTEGER NOT NULL
                           CHECK(delivery_delay_seconds BETWEEN 0 AND 86400),
  delivery_paused         INTEGER NOT NULL DEFAULT 0 CHECK(delivery_paused IN (0, 1)),
  retention_seconds        INTEGER NOT NULL
                           CHECK(retention_seconds BETWEEN 60 AND 1209600),
  max_message_bytes        INTEGER NOT NULL CHECK(max_message_bytes > 0),
  max_batch_messages       INTEGER NOT NULL CHECK(max_batch_messages > 0),
  max_batch_bytes          INTEGER NOT NULL CHECK(max_batch_bytes > 0),
  max_backlog_bytes        INTEGER NOT NULL CHECK(max_backlog_bytes > 0),
  created_at_ms            INTEGER NOT NULL,
  updated_at_ms            INTEGER NOT NULL,
  deleted_at_ms            INTEGER,
  CHECK(availability_code IS NULL OR
        length(availability_code) BETWEEN 1 AND 128),
  CHECK((availability = 'healthy') = (availability_code IS NULL)),
  CHECK((state = 'tombstoned') = (deleted_at_ms IS NOT NULL))
) STRICT;
CREATE UNIQUE INDEX queues_live_name
ON queues(account_id, name)
WHERE state != 'tombstoned';
CREATE INDEX queues_reconcile
ON queues(state, availability, updated_at_ms, id)
WHERE state IN ('creating', 'deleting') OR availability != 'healthy';
CREATE TABLE queue_producer_bindings (
  id                         TEXT PRIMARY KEY
                             CHECK(length(id) = 36 AND id = lower(id)),
  version_id              TEXT NOT NULL REFERENCES worker_versions(id),
  name                       TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 64),
  queue_id                   TEXT NOT NULL REFERENCES queues(id),
  queue_lifecycle_generation INTEGER NOT NULL CHECK(queue_lifecycle_generation >= 1),
  capability_version         INTEGER NOT NULL CHECK(capability_version >= 1),
  descriptor_sha256          BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms              INTEGER NOT NULL,
  UNIQUE(version_id, name)
) STRICT;
CREATE INDEX queue_producer_bindings_queue
ON queue_producer_bindings(queue_id, version_id, id);
CREATE TABLE queue_referrers (
  queue_id       TEXT NOT NULL REFERENCES queues(id),
  referrer_kind  TEXT NOT NULL CHECK(referrer_kind IN (
                   'producer_binding', 'consumer', 'dlq'
                 )),
  referrer_id    TEXT NOT NULL,
  created_at_ms  INTEGER NOT NULL,
  PRIMARY KEY(queue_id, referrer_kind, referrer_id)
) WITHOUT ROWID, STRICT;
CREATE TRIGGER queues_identity_update_guard
BEFORE UPDATE ON queues
WHEN OLD.id != NEW.id OR OLD.account_id != NEW.account_id OR
     OLD.lifecycle_generation != NEW.lifecycle_generation OR
     OLD.created_at_ms != NEW.created_at_ms OR
     (OLD.state = 'tombstoned' AND (
       NEW.state != OLD.state OR NEW.name != OLD.name OR
       NEW.config_generation != OLD.config_generation OR
       NEW.delivery_delay_seconds != OLD.delivery_delay_seconds OR
       NEW.delivery_paused != OLD.delivery_paused OR
       NEW.retention_seconds != OLD.retention_seconds OR
       NEW.max_message_bytes != OLD.max_message_bytes OR
       NEW.max_batch_messages != OLD.max_batch_messages OR
       NEW.max_batch_bytes != OLD.max_batch_bytes OR
       NEW.max_backlog_bytes != OLD.max_backlog_bytes OR
       NEW.deleted_at_ms != OLD.deleted_at_ms
     ))
BEGIN
  SELECT RAISE(ABORT, 'queue immutable identity invariant');
END;
CREATE TRIGGER queues_transition_guard
BEFORE UPDATE OF state ON queues
WHEN OLD.state != NEW.state AND NOT (
  (OLD.state = 'creating' AND NEW.state IN ('ready', 'deleting')) OR
  (OLD.state = 'ready' AND NEW.state = 'deleting') OR
  (OLD.state = 'deleting' AND NEW.state = 'tombstoned')
)
BEGIN
  SELECT RAISE(ABORT, 'queue lifecycle transition invariant');
END;
CREATE TRIGGER queues_config_update_guard
BEFORE UPDATE ON queues
WHEN (
  OLD.delivery_delay_seconds != NEW.delivery_delay_seconds OR
  OLD.retention_seconds != NEW.retention_seconds OR
  OLD.max_message_bytes != NEW.max_message_bytes OR
  OLD.max_batch_messages != NEW.max_batch_messages OR
  OLD.max_batch_bytes != NEW.max_batch_bytes OR
  OLD.max_backlog_bytes != NEW.max_backlog_bytes
) AND (
  OLD.state != 'ready' OR NEW.state != 'ready' OR
  NEW.config_generation != OLD.config_generation + 1 OR
  NEW.availability != 'degraded' OR
  NEW.availability_code != 'QUEUE_CONFIG_PENDING'
)
BEGIN
  SELECT RAISE(ABORT, 'queue config generation invariant');
END;
CREATE TRIGGER queues_config_generation_guard
BEFORE UPDATE ON queues
WHEN OLD.config_generation != NEW.config_generation AND NOT (
  OLD.state = 'ready' AND NEW.state = 'ready' AND
  NEW.config_generation = OLD.config_generation + 1 AND
  (OLD.delivery_delay_seconds != NEW.delivery_delay_seconds OR
   OLD.retention_seconds != NEW.retention_seconds OR
   OLD.max_message_bytes != NEW.max_message_bytes OR
   OLD.max_batch_messages != NEW.max_batch_messages OR
   OLD.max_batch_bytes != NEW.max_batch_bytes OR
   OLD.max_backlog_bytes != NEW.max_backlog_bytes) AND
  NEW.availability = 'degraded' AND
  NEW.availability_code = 'QUEUE_CONFIG_PENDING'
)
BEGIN
  SELECT RAISE(ABORT, 'queue config generation invariant');
END;
CREATE TRIGGER queues_rename_guard
BEFORE UPDATE OF name ON queues
WHEN OLD.name != NEW.name AND NOT (
  OLD.state = 'ready' AND NEW.state = 'ready' AND
  OLD.availability = 'healthy' AND NEW.availability = 'healthy' AND
  OLD.config_generation = NEW.config_generation
)
BEGIN
  SELECT RAISE(ABORT, 'queue rename lifecycle invariant');
END;
CREATE TRIGGER queues_delivery_pause_guard
BEFORE UPDATE OF delivery_paused ON queues
WHEN OLD.delivery_paused != NEW.delivery_paused AND NOT (
  OLD.state = 'ready' AND NEW.state = 'ready' AND
  OLD.availability = 'healthy' AND NEW.availability = 'healthy' AND
  OLD.config_generation = NEW.config_generation
)
BEGIN
  SELECT RAISE(ABORT, 'queue delivery pause invariant');
END;
CREATE TRIGGER queues_delete_referrer_guard
BEFORE UPDATE OF state ON queues
WHEN NEW.state = 'deleting' AND EXISTS (
  SELECT 1 FROM queue_referrers WHERE queue_id = OLD.id
)
BEGIN
  SELECT RAISE(ABORT, 'queue is referenced');
END;
CREATE TRIGGER queue_producer_bindings_insert_guard
BEFORE INSERT ON queue_producer_bindings
BEGIN
  SELECT CASE WHEN NEW.capability_version != 1
    THEN RAISE(ABORT, 'queue capability unsupported') END;
  SELECT CASE WHEN NEW.name GLOB '*[^A-Za-z0-9_$]*' OR
                   NEW.name GLOB '[0-9]*' OR NEW.name GLOB 'OPEN_COMPUTE_*' OR
                   NEW.name GLOB '__*'
    THEN RAISE(ABORT, 'queue binding name invalid') END;
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM worker_versions d
    JOIN workers w ON w.id = d.worker_id
    JOIN queues q ON q.id = NEW.queue_id
    WHERE d.id = NEW.version_id AND d.state = 'staging'
      AND w.account_id = q.account_id
      AND q.state = 'ready' AND q.availability = 'healthy'
      AND q.lifecycle_generation = NEW.queue_lifecycle_generation
  ) THEN RAISE(ABORT, 'queue binding authority invariant') END;
  SELECT CASE WHEN EXISTS (
    SELECT 1 FROM version_vars v
      WHERE v.version_id = NEW.version_id AND v.name = NEW.name
    UNION ALL
    SELECT 1 FROM version_secrets s
      WHERE s.version_id = NEW.version_id AND s.name = NEW.name
    UNION ALL
    SELECT 1 FROM version_bindings b
      WHERE b.version_id = NEW.version_id AND b.name = NEW.name
  ) THEN RAISE(ABORT, 'queue binding env conflict') END;
END;
CREATE TRIGGER version_bindings_queue_name_guard
BEFORE INSERT ON version_bindings
WHEN EXISTS (
  SELECT 1 FROM queue_producer_bindings q
  WHERE q.version_id = NEW.version_id AND q.name = NEW.name
)
BEGIN
  SELECT RAISE(ABORT, 'version binding env conflict');
END;
CREATE TRIGGER version_vars_queue_name_guard
BEFORE INSERT ON version_vars
WHEN EXISTS (
  SELECT 1 FROM queue_producer_bindings q
  WHERE q.version_id = NEW.version_id AND q.name = NEW.name
)
BEGIN
  SELECT RAISE(ABORT, 'version variable Queue env conflict');
END;
CREATE TRIGGER version_secrets_queue_name_guard
BEFORE INSERT ON version_secrets
WHEN EXISTS (
  SELECT 1 FROM queue_producer_bindings q
  WHERE q.version_id = NEW.version_id AND q.name = NEW.name
)
BEGIN
  SELECT RAISE(ABORT, 'version secret Queue env conflict');
END;
CREATE TRIGGER queue_producer_bindings_update_guard
BEFORE UPDATE ON queue_producer_bindings
BEGIN
  SELECT RAISE(ABORT, 'queue producer binding is immutable');
END;
CREATE TRIGGER queue_producer_bindings_delete_guard
BEFORE DELETE ON queue_producer_bindings
WHEN NOT EXISTS (
  SELECT 1 FROM worker_versions d
  WHERE d.id = OLD.version_id AND d.state IN ('staging', 'deleting')
)
BEGIN
  SELECT RAISE(ABORT, 'queue producer binding delete invariant');
END;
CREATE TRIGGER queue_producer_bindings_referrer_insert
AFTER INSERT ON queue_producer_bindings
BEGIN
  INSERT INTO queue_referrers(queue_id, referrer_kind, referrer_id, created_at_ms)
  VALUES (NEW.queue_id, 'producer_binding', NEW.id, NEW.created_at_ms);
END;
CREATE TRIGGER queue_producer_bindings_referrer_delete
AFTER DELETE ON queue_producer_bindings
BEGIN
  DELETE FROM queue_referrers
  WHERE queue_id = OLD.queue_id AND referrer_kind = 'producer_binding'
    AND referrer_id = OLD.id;
END;
CREATE TRIGGER queue_referrers_producer_insert_guard
BEFORE INSERT ON queue_referrers
WHEN NEW.referrer_kind = 'producer_binding' AND NOT EXISTS (
  SELECT 1 FROM queue_producer_bindings b
  WHERE b.id = NEW.referrer_id AND b.queue_id = NEW.queue_id
)
BEGIN
  SELECT RAISE(ABORT, 'orphan queue producer referrer');
END;
CREATE TRIGGER queue_referrers_producer_delete_guard
BEFORE DELETE ON queue_referrers
WHEN OLD.referrer_kind = 'producer_binding' AND EXISTS (
  SELECT 1 FROM queue_producer_bindings b
  JOIN worker_versions d ON d.id = b.version_id
  JOIN workers w ON w.id = d.worker_id
  WHERE b.id = OLD.referrer_id AND b.queue_id = OLD.queue_id
    AND w.deleted_at_ms IS NULL
)
BEGIN
  SELECT RAISE(ABORT, 'live queue producer referrer');
END;
CREATE TABLE version_queue_consumers (
  id                         TEXT PRIMARY KEY
                             CHECK(length(id) = 36 AND id = lower(id)),
  version_id              TEXT NOT NULL REFERENCES worker_versions(id),
  origin                     TEXT NOT NULL CHECK(origin IN ('version', 'api')),
  queue_id                   TEXT NOT NULL REFERENCES queues(id),
  queue_lifecycle_generation INTEGER NOT NULL CHECK(queue_lifecycle_generation >= 1),
  entrypoint                 TEXT CHECK(entrypoint IS NULL OR length(entrypoint) BETWEEN 1 AND 128),
  max_batch_size             INTEGER NOT NULL CHECK(max_batch_size BETWEEN 1 AND 100),
  max_batch_timeout_seconds  INTEGER NOT NULL CHECK(max_batch_timeout_seconds BETWEEN 0 AND 60),
  max_retries                INTEGER NOT NULL CHECK(max_retries BETWEEN 0 AND 100),
  retry_delay_seconds        INTEGER NOT NULL CHECK(retry_delay_seconds BETWEEN 0 AND 86400),
  max_concurrency            INTEGER NOT NULL CHECK(max_concurrency BETWEEN 1 AND 4096),
  dlq_queue_id               TEXT REFERENCES queues(id),
  dlq_lifecycle_generation   INTEGER,
  capability_version         INTEGER NOT NULL CHECK(capability_version = 1),
  descriptor_sha256          BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms              INTEGER NOT NULL,
  CHECK((dlq_queue_id IS NULL) = (dlq_lifecycle_generation IS NULL)),
  CHECK(dlq_lifecycle_generation IS NULL OR dlq_lifecycle_generation >= 1),
  CHECK(dlq_queue_id IS NULL OR dlq_queue_id != queue_id)
) STRICT;
CREATE UNIQUE INDEX version_queue_consumers_manifest_queue
ON version_queue_consumers(version_id, queue_id)
WHERE origin = 'version';
CREATE INDEX version_queue_consumers_queue
ON version_queue_consumers(queue_id, version_id, id);
CREATE TABLE queue_consumers (
  id                    TEXT PRIMARY KEY
                        CHECK(length(id) = 36 AND id = lower(id)),
  account_id            TEXT NOT NULL REFERENCES accounts(id),
  queue_id              TEXT NOT NULL REFERENCES queues(id),
  worker_id             TEXT NOT NULL REFERENCES workers(id),
  declaration_id        TEXT NOT NULL REFERENCES version_queue_consumers(id),
  version_id         TEXT NOT NULL REFERENCES worker_versions(id),
  pending_declaration_id TEXT REFERENCES version_queue_consumers(id),
  pending_version_id TEXT REFERENCES worker_versions(id),
  pending_worker_id       TEXT REFERENCES workers(id),
  consumer_generation   INTEGER NOT NULL CHECK(consumer_generation >= 1),
  state                 TEXT NOT NULL CHECK(state IN (
                          'activating', 'active', 'paused', 'updating',
                          'deleting', 'tombstoned'
                        )),
  availability          TEXT NOT NULL CHECK(availability IN (
                          'healthy', 'degraded', 'unavailable'
                        )),
  availability_code     TEXT,
  created_at_ms         INTEGER NOT NULL,
  updated_at_ms         INTEGER NOT NULL,
  deleted_at_ms         INTEGER,
  CHECK(availability_code IS NULL OR length(availability_code) BETWEEN 1 AND 128),
  CHECK((pending_declaration_id IS NULL) = (pending_version_id IS NULL)),
  CHECK((pending_declaration_id IS NULL) = (pending_worker_id IS NULL)),
  CHECK(pending_version_id IS NULL OR state IN ('updating', 'deleting')),
  CHECK((state = 'tombstoned') = (deleted_at_ms IS NOT NULL)),
  CHECK((availability = 'healthy') = (availability_code IS NULL))
) STRICT;
CREATE UNIQUE INDEX queue_one_live_consumer
ON queue_consumers(queue_id)
WHERE state != 'tombstoned';
CREATE INDEX queue_consumers_reconcile
ON queue_consumers(state, availability, updated_at_ms, id)
WHERE state IN ('activating', 'updating', 'deleting') OR availability != 'healthy';
CREATE TRIGGER version_queue_consumers_insert_guard
BEFORE INSERT ON version_queue_consumers
BEGIN
  SELECT CASE WHEN NEW.capability_version != 1
    THEN RAISE(ABORT, 'queue consumer capability unsupported') END;
  SELECT CASE WHEN NEW.entrypoint IS NOT NULL AND (
    NEW.entrypoint GLOB '*[^A-Za-z0-9_$]*' OR
    NEW.entrypoint GLOB '[0-9]*'
  ) THEN RAISE(ABORT, 'queue consumer entrypoint invalid') END;
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM worker_versions d
    JOIN workers w ON w.id = d.worker_id
    JOIN queues q ON q.id = NEW.queue_id
    WHERE d.id = NEW.version_id AND (
      (NEW.origin = 'version' AND d.state = 'staging') OR
      (NEW.origin = 'api' AND d.state = 'ready' AND EXISTS (
        SELECT 1 FROM worker_deployments p
        WHERE p.id = w.active_deployment_id AND p.version_id = d.id
      ))
    )
      AND w.account_id = q.account_id
      AND q.state = 'ready' AND q.availability = 'healthy'
      AND q.lifecycle_generation = NEW.queue_lifecycle_generation
  ) THEN RAISE(ABORT, 'queue consumer authority invariant') END;
  SELECT CASE WHEN NEW.dlq_queue_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM worker_versions d
    JOIN workers w ON w.id = d.worker_id
    JOIN queues q ON q.id = NEW.dlq_queue_id
    WHERE d.id = NEW.version_id AND (
      (NEW.origin = 'version' AND d.state = 'staging') OR
      (NEW.origin = 'api' AND d.state = 'ready' AND EXISTS (
        SELECT 1 FROM worker_deployments p
        WHERE p.id = w.active_deployment_id AND p.version_id = d.id
      ))
    )
      AND w.account_id = q.account_id
      AND q.state = 'ready' AND q.availability = 'healthy'
      AND q.lifecycle_generation = NEW.dlq_lifecycle_generation
  ) THEN RAISE(ABORT, 'queue consumer DLQ authority invariant') END;
END;
CREATE TRIGGER version_queue_consumers_update_guard
BEFORE UPDATE ON version_queue_consumers
BEGIN
  SELECT RAISE(ABORT, 'queue consumer declaration is immutable');
END;
CREATE TRIGGER version_queue_consumers_delete_guard
BEFORE DELETE ON version_queue_consumers
WHEN OLD.origin = 'version' AND NOT EXISTS (
  SELECT 1 FROM worker_versions d
  WHERE d.id = OLD.version_id AND d.state IN ('staging', 'rejected', 'deleting')
)
BEGIN
  SELECT RAISE(ABORT, 'queue consumer declaration delete invariant');
END;
CREATE TRIGGER version_queue_consumers_referrers_insert
AFTER INSERT ON version_queue_consumers
BEGIN
  INSERT INTO queue_referrers(queue_id, referrer_kind, referrer_id, created_at_ms)
  VALUES (NEW.queue_id, 'consumer', NEW.id, NEW.created_at_ms);
  INSERT INTO queue_referrers(queue_id, referrer_kind, referrer_id, created_at_ms)
  SELECT NEW.dlq_queue_id, 'dlq', NEW.id, NEW.created_at_ms
  WHERE NEW.dlq_queue_id IS NOT NULL;
END;
CREATE TRIGGER version_queue_consumers_referrers_delete
AFTER DELETE ON version_queue_consumers
BEGIN
  DELETE FROM queue_referrers
  WHERE queue_id = OLD.queue_id AND referrer_kind = 'consumer' AND referrer_id = OLD.id;
  DELETE FROM queue_referrers
  WHERE queue_id = OLD.dlq_queue_id AND referrer_kind = 'dlq' AND referrer_id = OLD.id;
END;
CREATE TRIGGER queue_consumer_referrer_delete_guard
BEFORE DELETE ON queue_referrers
WHEN OLD.referrer_kind IN ('consumer', 'dlq') AND EXISTS (
  SELECT 1 FROM version_queue_consumers c
  WHERE c.id = OLD.referrer_id AND c.origin = 'version' AND (
    (OLD.referrer_kind = 'consumer' AND c.queue_id = OLD.queue_id) OR
    (OLD.referrer_kind = 'dlq' AND c.dlq_queue_id = OLD.queue_id)
  )
)
BEGIN
  SELECT RAISE(ABORT, 'live queue consumer referrer');
END;
CREATE TRIGGER queue_consumers_insert_guard
BEFORE INSERT ON queue_consumers
BEGIN
  SELECT CASE WHEN NEW.state != 'activating' OR NEW.consumer_generation != 1 OR
                   NEW.availability != 'degraded' OR
                   NEW.availability_code != 'QUEUE_CONSUMER_PROJECTION_PENDING'
    THEN RAISE(ABORT, 'queue consumer activation invariant') END;
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM version_queue_consumers c
    JOIN worker_versions d ON d.id = c.version_id
    JOIN workers w ON w.id = d.worker_id
    WHERE c.id = NEW.declaration_id AND c.version_id = NEW.version_id
      AND c.queue_id = NEW.queue_id AND d.state = 'ready'
      AND d.worker_id = NEW.worker_id AND w.account_id = NEW.account_id
  ) THEN RAISE(ABORT, 'queue consumer live authority invariant') END;
END;
CREATE TRIGGER queue_consumers_identity_guard
BEFORE UPDATE OF id, account_id, queue_id, created_at_ms ON queue_consumers
BEGIN
  SELECT RAISE(ABORT, 'queue consumer identity is immutable');
END;
CREATE TRIGGER queue_consumers_transition_guard
BEFORE UPDATE OF state ON queue_consumers
WHEN OLD.state != NEW.state AND NOT (
  (OLD.state = 'activating' AND NEW.state IN ('active', 'paused', 'deleting')) OR
  (OLD.state = 'active' AND NEW.state IN ('paused', 'updating', 'deleting')) OR
  (OLD.state = 'paused' AND NEW.state IN ('active', 'updating', 'deleting')) OR
  (OLD.state = 'updating' AND NEW.state IN ('active', 'paused', 'deleting')) OR
  (OLD.state = 'deleting' AND NEW.state = 'tombstoned')
)
BEGIN
  SELECT RAISE(ABORT, 'queue consumer transition invariant');
END;
CREATE TRIGGER queue_consumers_generation_guard
BEFORE UPDATE ON queue_consumers
WHEN OLD.consumer_generation != NEW.consumer_generation AND NOT (
  NEW.consumer_generation = OLD.consumer_generation + 1 AND
  NEW.state = 'updating' AND NEW.availability = 'degraded' AND
  NEW.pending_declaration_id IS NOT NULL AND NEW.pending_version_id IS NOT NULL AND
  NEW.pending_worker_id IS NOT NULL AND
  NEW.availability_code IN (
    'QUEUE_CONSUMER_DRAINING', 'QUEUE_CONSUMER_DRAINING_PAUSED'
  )
)
BEGIN
  SELECT RAISE(ABORT, 'queue consumer generation invariant');
END;
CREATE TRIGGER queue_consumers_pending_target_guard
BEFORE UPDATE OF pending_declaration_id, pending_version_id, pending_worker_id ON queue_consumers
WHEN NOT (
  (OLD.pending_declaration_id IS NULL AND OLD.pending_version_id IS NULL AND OLD.pending_worker_id IS NULL AND
   NEW.pending_declaration_id IS NOT NULL AND NEW.pending_version_id IS NOT NULL AND NEW.pending_worker_id IS NOT NULL AND
   OLD.state IN ('active', 'paused') AND NEW.state = 'updating' AND
   NEW.consumer_generation = OLD.consumer_generation + 1 AND EXISTS (
     SELECT 1 FROM version_queue_consumers c
     JOIN worker_versions d ON d.id = c.version_id
     WHERE c.id = NEW.pending_declaration_id
       AND c.version_id = NEW.pending_version_id
       AND c.queue_id = NEW.queue_id AND d.worker_id = NEW.pending_worker_id
       AND d.state = 'ready'
   )) OR
  (OLD.pending_declaration_id IS NOT NULL AND OLD.pending_version_id IS NOT NULL AND OLD.pending_worker_id IS NOT NULL AND
   NEW.pending_declaration_id IS NULL AND NEW.pending_version_id IS NULL AND NEW.pending_worker_id IS NULL AND
   OLD.state IN ('updating', 'deleting') AND NEW.state IN ('updating', 'tombstoned')) OR
  (OLD.pending_declaration_id IS NEW.pending_declaration_id AND
   OLD.pending_version_id IS NEW.pending_version_id AND
   OLD.pending_worker_id IS NEW.pending_worker_id)
)
BEGIN
  SELECT RAISE(ABORT, 'queue consumer pending target invariant');
END;
CREATE TRIGGER queue_consumers_target_guard
BEFORE UPDATE OF declaration_id, version_id, worker_id ON queue_consumers
WHEN OLD.declaration_id != NEW.declaration_id OR OLD.version_id != NEW.version_id OR OLD.worker_id != NEW.worker_id
BEGIN
  SELECT CASE WHEN OLD.state != 'updating' OR NEW.state != 'updating' OR
                   OLD.consumer_generation != NEW.consumer_generation OR
                   NEW.declaration_id != OLD.pending_declaration_id OR
                   NEW.version_id != OLD.pending_version_id OR
                   NEW.worker_id != OLD.pending_worker_id OR
                   NEW.pending_declaration_id IS NOT NULL OR
                   NEW.pending_version_id IS NOT NULL OR
                   NEW.pending_worker_id IS NOT NULL OR NOT EXISTS (
    SELECT 1 FROM version_queue_consumers c
    JOIN worker_versions d ON d.id = c.version_id
    WHERE c.id = NEW.declaration_id AND c.version_id = NEW.version_id
      AND c.queue_id = NEW.queue_id AND d.worker_id = NEW.worker_id AND d.state = 'ready'
  ) THEN RAISE(ABORT, 'queue consumer target invariant') END;
END;
CREATE TRIGGER queue_consumers_tombstone_guard
BEFORE UPDATE ON queue_consumers
WHEN OLD.state = 'tombstoned'
BEGIN
  SELECT RAISE(ABORT, 'queue consumer tombstone is immutable');
END;
CREATE TRIGGER queue_consumers_version_referrer_insert
AFTER INSERT ON queue_consumers
BEGIN
  INSERT INTO version_referrers(version_id, kind, ref_id, created_at_ms)
  VALUES (NEW.version_id, 'queue_consumer', NEW.id, NEW.created_at_ms);
END;
CREATE TRIGGER queue_consumers_version_referrer_update
AFTER UPDATE OF version_id ON queue_consumers
WHEN OLD.version_id != NEW.version_id
BEGIN
  DELETE FROM version_referrers
  WHERE version_id = OLD.version_id AND kind = 'queue_consumer' AND ref_id = OLD.id;
  INSERT INTO version_referrers(version_id, kind, ref_id, created_at_ms)
  VALUES (NEW.version_id, 'queue_consumer', NEW.id, NEW.updated_at_ms);
END;
CREATE TRIGGER queue_consumers_pending_referrer_insert
AFTER UPDATE OF pending_version_id ON queue_consumers
WHEN OLD.pending_version_id IS NULL AND NEW.pending_version_id IS NOT NULL
BEGIN
  INSERT INTO version_referrers(version_id, kind, ref_id, created_at_ms)
  VALUES (NEW.pending_version_id, 'queue_consumer_pending', NEW.id, NEW.updated_at_ms);
END;
CREATE TRIGGER queue_consumers_pending_referrer_delete
AFTER UPDATE OF pending_version_id ON queue_consumers
WHEN OLD.pending_version_id IS NOT NULL AND NEW.pending_version_id IS NULL
BEGIN
  DELETE FROM version_referrers
  WHERE version_id = OLD.pending_version_id
    AND kind = 'queue_consumer_pending' AND ref_id = OLD.id;
END;
CREATE TRIGGER queue_consumers_version_referrer_tombstone
AFTER UPDATE OF state ON queue_consumers
WHEN NEW.state = 'tombstoned'
BEGIN
  DELETE FROM version_referrers
  WHERE version_id = NEW.version_id AND kind = 'queue_consumer' AND ref_id = NEW.id;
END;
CREATE TRIGGER queue_consumers_api_referrers_target_switch
AFTER UPDATE OF declaration_id ON queue_consumers
WHEN OLD.declaration_id != NEW.declaration_id AND EXISTS (
  SELECT 1 FROM version_queue_consumers c WHERE c.id = OLD.declaration_id AND c.origin = 'api'
)
BEGIN
  DELETE FROM queue_referrers
  WHERE referrer_id = OLD.declaration_id AND referrer_kind IN ('consumer', 'dlq');
END;
CREATE TRIGGER queue_consumers_api_referrers_tombstone
AFTER UPDATE OF state ON queue_consumers
WHEN NEW.state = 'tombstoned'
BEGIN
  DELETE FROM queue_referrers
  WHERE referrer_id IN (OLD.declaration_id, OLD.pending_declaration_id)
    AND referrer_kind IN ('consumer', 'dlq')
    AND EXISTS (
      SELECT 1 FROM version_queue_consumers c
      WHERE c.id = queue_referrers.referrer_id AND c.origin = 'api'
    );
END;
CREATE TABLE version_cron_configs (
  version_id      TEXT PRIMARY KEY REFERENCES worker_versions(id),
  capability_version INTEGER NOT NULL CHECK(capability_version = 1),
  descriptor_sha256  BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms      INTEGER NOT NULL
) STRICT;
CREATE TABLE version_cron_declarations (
  id                  TEXT PRIMARY KEY
                      CHECK(length(id) = 36 AND id = lower(id)),
  version_id       TEXT NOT NULL REFERENCES worker_versions(id),
  expression          TEXT NOT NULL CHECK(length(expression) BETWEEN 1 AND 256),
  expression_sha256   BLOB NOT NULL CHECK(length(expression_sha256) = 32),
  parser_version      INTEGER NOT NULL CHECK(parser_version >= 1),
  scheduled_handler   INTEGER NOT NULL CHECK(scheduled_handler IN (0, 1)),
  workflow_bindings_json BLOB NOT NULL CHECK(
                         length(workflow_bindings_json) BETWEEN 2 AND 16384
                       ),
  created_at_ms       INTEGER NOT NULL,
  UNIQUE(version_id, expression)
) STRICT;
CREATE TABLE cron_activations (
  id                    TEXT PRIMARY KEY
                        CHECK(length(id) = 36 AND id = lower(id)),
  account_id            TEXT NOT NULL REFERENCES accounts(id),
  worker_id             TEXT NOT NULL REFERENCES workers(id),
  version_id         TEXT NOT NULL REFERENCES worker_versions(id),
  expression            TEXT NOT NULL CHECK(length(expression) BETWEEN 1 AND 256),
  expression_sha256     BLOB NOT NULL CHECK(length(expression_sha256) = 32),
  parser_version        INTEGER NOT NULL CHECK(parser_version >= 1),
  scheduled_handler     INTEGER NOT NULL CHECK(scheduled_handler IN (0, 1)),
  workflow_bindings_json BLOB NOT NULL CHECK(
                           length(workflow_bindings_json) BETWEEN 2 AND 16384
                         ),
  activation_generation INTEGER NOT NULL CHECK(activation_generation >= 1),
  state                 TEXT NOT NULL CHECK(state IN (
                          'staging', 'active', 'retiring', 'tombstoned'
                        )),
  availability          TEXT NOT NULL CHECK(availability IN (
                          'healthy', 'degraded', 'unavailable'
                        )),
  availability_code     TEXT,
  created_at_ms         INTEGER NOT NULL,
  updated_at_ms         INTEGER NOT NULL,
  deleted_at_ms         INTEGER,
  UNIQUE(worker_id, activation_generation, expression),
  CHECK(availability_code IS NULL OR length(availability_code) BETWEEN 1 AND 128),
  CHECK((state = 'tombstoned') = (deleted_at_ms IS NOT NULL)),
  CHECK((availability = 'healthy') = (availability_code IS NULL))
) STRICT;
CREATE INDEX cron_activations_reconcile
ON cron_activations(state, availability, updated_at_ms, id)
WHERE state IN ('staging', 'retiring') OR availability != 'healthy';
CREATE TRIGGER version_cron_configs_insert_guard
BEFORE INSERT ON version_cron_configs
WHEN NOT EXISTS (
  SELECT 1 FROM worker_versions d
  WHERE d.id = NEW.version_id AND d.state = 'staging'
)
BEGIN
  SELECT RAISE(ABORT, 'cron config authority invariant');
END;
CREATE TRIGGER version_cron_configs_update_guard
BEFORE UPDATE ON version_cron_configs
BEGIN
  SELECT RAISE(ABORT, 'cron version config is immutable');
END;
CREATE TRIGGER version_cron_configs_delete_guard
BEFORE DELETE ON version_cron_configs
WHEN NOT EXISTS (
  SELECT 1 FROM worker_versions d
  WHERE d.id = OLD.version_id AND d.state IN ('staging', 'rejected', 'deleting')
)
BEGIN
  SELECT RAISE(ABORT, 'cron version config delete invariant');
END;
CREATE TRIGGER version_cron_declarations_insert_guard
BEFORE INSERT ON version_cron_declarations
WHEN NOT EXISTS (
  SELECT 1 FROM version_cron_configs c
  JOIN worker_versions d ON d.id = c.version_id
  WHERE c.version_id = NEW.version_id AND d.state = 'staging'
)
BEGIN
  SELECT RAISE(ABORT, 'cron declaration authority invariant');
END;
CREATE TRIGGER version_cron_declarations_update_guard
BEFORE UPDATE ON version_cron_declarations
BEGIN
  SELECT RAISE(ABORT, 'cron declaration is immutable');
END;
CREATE TRIGGER version_cron_declarations_delete_guard
BEFORE DELETE ON version_cron_declarations
WHEN NOT EXISTS (
  SELECT 1 FROM worker_versions d
  WHERE d.id = OLD.version_id AND d.state IN ('staging', 'rejected', 'deleting')
)
BEGIN
  SELECT RAISE(ABORT, 'cron declaration delete invariant');
END;
CREATE TRIGGER cron_activations_insert_guard
BEFORE INSERT ON cron_activations
BEGIN
  SELECT CASE WHEN NEW.state != 'staging' OR NEW.availability != 'degraded' OR
                   NEW.availability_code != 'CRON_PROJECTION_PENDING'
    THEN RAISE(ABORT, 'cron activation staging invariant') END;
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM worker_versions d JOIN workers w ON w.id = d.worker_id
    WHERE d.id = NEW.version_id AND d.worker_id = NEW.worker_id
      AND d.state = 'ready' AND w.account_id = NEW.account_id
  ) THEN RAISE(ABORT, 'cron activation authority invariant') END;
END;
CREATE TRIGGER cron_activations_identity_guard
BEFORE UPDATE OF id, account_id, worker_id, expression, expression_sha256,
  parser_version, scheduled_handler, workflow_bindings_json,
  activation_generation, created_at_ms ON cron_activations
BEGIN
  SELECT RAISE(ABORT, 'cron activation identity is immutable');
END;
CREATE TRIGGER cron_activations_target_guard
BEFORE UPDATE OF version_id ON cron_activations
BEGIN
  SELECT RAISE(ABORT, 'cron activation target is immutable');
END;
CREATE TRIGGER cron_activations_transition_guard
BEFORE UPDATE OF state ON cron_activations
WHEN OLD.state != NEW.state AND NOT (
  (OLD.state = 'staging' AND NEW.state IN ('active', 'retiring')) OR
  (OLD.state = 'active' AND NEW.state = 'retiring') OR
  (OLD.state = 'retiring' AND NEW.state = 'tombstoned')
)
BEGIN
  SELECT RAISE(ABORT, 'cron activation transition invariant');
END;
CREATE TRIGGER cron_activations_tombstone_guard
BEFORE UPDATE ON cron_activations
WHEN OLD.state = 'tombstoned'
BEGIN
  SELECT RAISE(ABORT, 'cron activation tombstone is immutable');
END;
CREATE TRIGGER cron_activations_referrer_insert
AFTER INSERT ON cron_activations
BEGIN
  INSERT INTO version_referrers(version_id, kind, ref_id, created_at_ms)
  VALUES (NEW.version_id, 'cron_activation', NEW.id, NEW.created_at_ms);
END;
CREATE TRIGGER cron_activations_referrer_tombstone
AFTER UPDATE OF state ON cron_activations
WHEN NEW.state = 'tombstoned'
BEGIN
  DELETE FROM version_referrers
  WHERE version_id = NEW.version_id AND kind = 'cron_activation' AND ref_id = NEW.id;
END;
CREATE TABLE workflow_bindings (
  id TEXT PRIMARY KEY,
  version_id TEXT NOT NULL REFERENCES worker_versions(id),
  name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 64),
  definition_id TEXT NOT NULL REFERENCES workflow_definitions(id),
  definition_lifecycle_generation INTEGER NOT NULL CHECK(definition_lifecycle_generation >= 1),
  class_name TEXT NOT NULL CHECK(length(class_name) BETWEEN 1 AND 128),
  reservation_owner TEXT CHECK(reservation_owner IS NULL OR length(reservation_owner) BETWEEN 1 AND 128),
  reservation_fence INTEGER CHECK(reservation_fence IS NULL OR reservation_fence >= 1),
  capability_version INTEGER NOT NULL CHECK(capability_version = 1),
  schedules_json BLOB NOT NULL CHECK(length(schedules_json) BETWEEN 2 AND 32768),
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms INTEGER NOT NULL,
  UNIQUE(version_id,name),
  CHECK((reservation_owner IS NULL) = (reservation_fence IS NULL))
) STRICT;
CREATE TABLE workflow_binding_operations (
  operation_id TEXT PRIMARY KEY,
  binding_id TEXT NOT NULL REFERENCES workflow_bindings(id),
  kind TEXT NOT NULL,
  fingerprint BLOB NOT NULL CHECK(length(fingerprint)=32),
  request_json BLOB NOT NULL CHECK(length(request_json)<=2097152),
  state TEXT NOT NULL CHECK(state IN ('prepared','applied')),
  response_json BLOB,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  CHECK((state='applied')=(response_json IS NOT NULL))
) STRICT;
CREATE TABLE workflow_binding_operation_locks (
  binding_id TEXT NOT NULL REFERENCES workflow_bindings(id),
  operation_id TEXT PRIMARY KEY REFERENCES workflow_binding_operations(operation_id),
  created_at_ms INTEGER NOT NULL
) STRICT;
CREATE TRIGGER workflow_binding_operation_immutable BEFORE UPDATE OF operation_id,binding_id,kind,
  fingerprint,request_json,created_at_ms ON workflow_binding_operations
BEGIN SELECT RAISE(ABORT,'workflow binding operation identity is immutable'); END;
CREATE TRIGGER workflow_binding_operation_transition BEFORE UPDATE ON workflow_binding_operations
WHEN OLD.state!='prepared' OR NEW.state!='applied' OR OLD.response_json IS NOT NULL OR NEW.response_json IS NULL
  OR NOT EXISTS(SELECT 1 FROM workflow_binding_operation_locks l
    WHERE l.binding_id=OLD.binding_id AND l.operation_id=OLD.operation_id)
BEGIN SELECT RAISE(ABORT,'workflow binding operation transition'); END;
CREATE TRIGGER workflow_binding_operation_lock_guard BEFORE INSERT ON workflow_binding_operation_locks
WHEN NOT EXISTS(SELECT 1 FROM workflow_binding_operations o WHERE o.operation_id=NEW.operation_id
  AND o.binding_id=NEW.binding_id AND o.state='prepared' AND o.created_at_ms=NEW.created_at_ms)
BEGIN SELECT RAISE(ABORT,'workflow binding operation lock authority'); END;
CREATE TRIGGER workflow_binding_operation_unlock_guard BEFORE DELETE ON workflow_binding_operation_locks
WHEN NOT EXISTS(SELECT 1 FROM workflow_binding_operations o WHERE o.operation_id=OLD.operation_id
  AND o.binding_id=OLD.binding_id AND o.state='applied')
BEGIN SELECT RAISE(ABORT,'workflow binding operation is unfinished'); END;
CREATE TABLE workflow_definitions (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 64),
  state TEXT NOT NULL CHECK(state IN ('creating','ready','deleting','tombstoned')),
  availability TEXT NOT NULL CHECK(availability IN ('healthy','degraded','unavailable')),
  availability_code TEXT,
  lifecycle_generation INTEGER NOT NULL CHECK(lifecycle_generation >= 1),
  reserved_class_name TEXT CHECK(reserved_class_name IS NULL OR length(reserved_class_name) BETWEEN 1 AND 128),
  reservation_owner TEXT CHECK(reservation_owner IS NULL OR length(reservation_owner) BETWEEN 1 AND 128),
  reservation_fence INTEGER NOT NULL DEFAULT 0 CHECK(reservation_fence >= 0),
  reservation_state TEXT CHECK(reservation_state IS NULL OR reservation_state IN ('reserved','bound')),
  reservation_created_definition INTEGER CHECK(reservation_created_definition IS NULL OR reservation_created_definition IN (0,1)),
  delete_fence INTEGER NOT NULL DEFAULT 0 CHECK(delete_fence >= 0),
  current_version_id TEXT REFERENCES workflow_versions(id) DEFERRABLE INITIALLY DEFERRED,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  deleted_at_ms INTEGER,
  CHECK((state = 'tombstoned') = (deleted_at_ms IS NOT NULL)),
  CHECK((availability = 'healthy') = (availability_code IS NULL)),
  CHECK((reserved_class_name IS NULL AND reservation_owner IS NULL AND reservation_state IS NULL
         AND reservation_created_definition IS NULL)
     OR (reserved_class_name IS NOT NULL AND reservation_owner IS NOT NULL AND reservation_fence >= 1
         AND reservation_state IS NOT NULL AND reservation_created_definition IS NOT NULL
         AND state IN ('creating','ready')
         AND (reservation_created_definition=0 OR (state='creating' AND current_version_id IS NULL)))),
  CHECK((state IN ('creating','ready') AND delete_fence=0)
    OR (state IN ('deleting','tombstoned') AND delete_fence>=1))
) STRICT;
CREATE TABLE workflow_instance_operations (
  operation_id TEXT PRIMARY KEY,
  instance_id TEXT NOT NULL UNIQUE REFERENCES workflow_instance_referrers(instance_id) ON DELETE CASCADE,
  creation_nonce BLOB NOT NULL CHECK(length(creation_nonce)=32),
  expected_generation INTEGER NOT NULL CHECK(expected_generation>=1),
  target_generation INTEGER NOT NULL CHECK(target_generation>=1),
  kind TEXT NOT NULL CHECK(kind IN ('restart','purge')),
  restart_from_name TEXT CHECK(restart_from_name IS NULL OR length(CAST(restart_from_name AS BLOB)) BETWEEN 1 AND 256),
  restart_from_count INTEGER CHECK(restart_from_count IS NULL OR restart_from_count BETWEEN 1 AND 1024),
  restart_from_kind TEXT CHECK(restart_from_kind IS NULL OR restart_from_kind IN ('do','sleep','waitForEvent')),
  prior_ref_state TEXT NOT NULL CHECK(prior_ref_state IN ('live','retained')),
  applied INTEGER NOT NULL DEFAULT 0 CHECK(applied IN (0,1)),
  created_at_ms INTEGER NOT NULL, operation_sequence INTEGER NOT NULL DEFAULT 1 CHECK(operation_sequence>=1),
  CHECK((kind='restart' AND expected_generation<9223372036854775807 AND target_generation=expected_generation+1)
     OR (kind='purge' AND target_generation=expected_generation AND prior_ref_state='retained')),
  CHECK((kind='purge' AND restart_from_name IS NULL AND restart_from_count IS NULL AND restart_from_kind IS NULL)
     OR (kind='restart' AND ((restart_from_name IS NULL AND restart_from_count IS NULL AND restart_from_kind IS NULL)
       OR (restart_from_name IS NOT NULL AND restart_from_count IS NOT NULL))))
) STRICT;
CREATE TABLE workflow_instance_referrers (
  instance_id TEXT PRIMARY KEY,
  definition_id TEXT NOT NULL REFERENCES workflow_definitions(id),
  definition_name TEXT NOT NULL,
  external_instance_id TEXT NOT NULL CHECK(length(external_instance_id) BETWEEN 1 AND 100),
  workflow_version_id TEXT NOT NULL REFERENCES workflow_versions(id),
  worker_version_id TEXT NOT NULL REFERENCES worker_versions(id),
  instance_generation INTEGER NOT NULL CHECK(instance_generation >= 1),
  creation_nonce BLOB NOT NULL CHECK(length(creation_nonce) = 32),
  creation_operation_id TEXT NOT NULL UNIQUE,
  creation_batch_id TEXT NOT NULL,
  state TEXT NOT NULL CHECK(state IN ('creating','live','retained','restarting','releasing','released')),
  trigger_cron TEXT CHECK(trigger_cron IS NULL OR length(trigger_cron) BETWEEN 1 AND 256),
  trigger_scheduled_time_ms INTEGER CHECK(trigger_scheduled_time_ms IS NULL OR trigger_scheduled_time_ms >= 0),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  released_at_ms INTEGER, operation_sequence INTEGER NOT NULL DEFAULT 0 CHECK(operation_sequence>=0),
  UNIQUE(definition_id,external_instance_id),
  CHECK((state = 'released') = (released_at_ms IS NOT NULL)),
  CHECK((trigger_cron IS NULL) = (trigger_scheduled_time_ms IS NULL))
) STRICT;
CREATE INDEX workflow_instance_creation_batch ON workflow_instance_referrers(creation_batch_id);
CREATE TABLE workflow_referrers (
  definition_id TEXT NOT NULL REFERENCES workflow_definitions(id),
  referrer_kind TEXT NOT NULL CHECK(referrer_kind IN ('binding','instance')),
  referrer_id TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  PRIMARY KEY(definition_id,referrer_kind,referrer_id)
) WITHOUT ROWID, STRICT;
CREATE TABLE workflow_versions (
  id TEXT PRIMARY KEY,
  definition_id TEXT NOT NULL REFERENCES workflow_definitions(id),
  version_number INTEGER NOT NULL CHECK(version_number > 0),
  state TEXT NOT NULL CHECK(state IN ('staging','validating','ready','rejected','deleting','tombstoned')),
  worker_id TEXT NOT NULL REFERENCES workers(id),
  worker_version_id TEXT NOT NULL REFERENCES worker_versions(id),
  class_name TEXT NOT NULL CHECK(length(class_name) BETWEEN 1 AND 128),
  reservation_owner TEXT CHECK(reservation_owner IS NULL OR length(reservation_owner) BETWEEN 1 AND 128),
  reservation_fence INTEGER CHECK(reservation_fence IS NULL OR reservation_fence >= 1),
  worker_code_sha256 BLOB NOT NULL CHECK(length(worker_code_sha256) = 32),
  loader_schema_version INTEGER NOT NULL CHECK(loader_schema_version > 0),
  capability_version INTEGER NOT NULL CHECK(capability_version = 1),
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms INTEGER NOT NULL,
  ready_at_ms INTEGER,
  rejected_at_ms INTEGER,
  rejection_code TEXT,
  deleted_at_ms INTEGER,
  UNIQUE(definition_id,version_number),
  CHECK((state = 'tombstoned') = (deleted_at_ms IS NOT NULL)),
  CHECK(state != 'ready' OR ready_at_ms IS NOT NULL),
  CHECK((reservation_owner IS NULL) = (reservation_fence IS NULL))
) STRICT;
CREATE UNIQUE INDEX workflow_definitions_live_name ON workflow_definitions(account_id,name)
WHERE state != 'tombstoned';
CREATE INDEX workflow_instance_operations_reconcile ON workflow_instance_operations(created_at_ms,operation_id);
CREATE INDEX workflow_instance_referrers_reconcile ON workflow_instance_referrers(state,updated_at_ms,instance_id);
CREATE TRIGGER workflow_binding_add_ref AFTER INSERT ON workflow_bindings
BEGIN INSERT INTO workflow_referrers VALUES(NEW.definition_id,'binding',NEW.id,NEW.created_at_ms); END;
CREATE TRIGGER workflow_binding_delete_guard BEFORE DELETE ON workflow_bindings
WHEN NOT EXISTS (SELECT 1 FROM worker_versions WHERE id = OLD.version_id AND state IN ('staging','rejected','deleting'))
BEGIN SELECT RAISE(ABORT,'workflow binding version is immutable'); END;
CREATE TRIGGER workflow_binding_immutable BEFORE UPDATE ON workflow_bindings
BEGIN SELECT RAISE(ABORT,'workflow binding is immutable'); END;
CREATE TRIGGER workflow_binding_insert_guard BEFORE INSERT ON workflow_bindings
BEGIN
  SELECT CASE WHEN NEW.name GLOB '*[^A-Za-z0-9_$]*' OR NEW.name GLOB '[0-9]*'
    OR NEW.name GLOB 'OPEN_COMPUTE_*' OR NEW.name GLOB '__*'
    THEN RAISE(ABORT,'workflow binding name') END;
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM worker_versions d JOIN workers w ON w.id = d.worker_id
    JOIN workflow_definitions f ON f.account_id = w.account_id
    WHERE d.id = NEW.version_id AND d.state = 'staging' AND f.id = NEW.definition_id
      AND f.lifecycle_generation = NEW.definition_lifecycle_generation
      AND (((NEW.reservation_owner IS NOT NULL AND NEW.reservation_fence IS NOT NULL)
            AND f.state IN ('creating','ready') AND f.reserved_class_name = NEW.class_name
            AND f.reservation_owner = NEW.reservation_owner
            AND f.reservation_fence = NEW.reservation_fence
            AND f.reservation_state IN ('reserved','bound'))
        OR (NEW.reservation_owner IS NULL AND NEW.reservation_fence IS NULL
            AND f.state = 'ready' AND f.availability = 'healthy'
            AND EXISTS(SELECT 1 FROM workflow_versions v
              WHERE v.id = f.current_version_id AND v.definition_id = f.id
                AND v.state = 'ready' AND v.class_name = NEW.class_name)))
  ) THEN RAISE(ABORT,'workflow binding authority') END;
  SELECT CASE WHEN EXISTS (
    SELECT 1 FROM version_vars WHERE version_id = NEW.version_id AND name = NEW.name
    UNION ALL SELECT 1 FROM version_secrets WHERE version_id = NEW.version_id AND name = NEW.name
    UNION ALL SELECT 1 FROM version_bindings WHERE version_id = NEW.version_id AND name = NEW.name
    UNION ALL SELECT 1 FROM queue_producer_bindings WHERE version_id = NEW.version_id AND name = NEW.name
  ) THEN RAISE(ABORT,'workflow binding name conflict') END;
END;
CREATE TRIGGER workflow_binding_marks_reservation_bound AFTER INSERT ON workflow_bindings
WHEN NEW.reservation_owner IS NOT NULL AND NEW.reservation_fence IS NOT NULL
BEGIN
  UPDATE workflow_definitions SET reservation_state='bound',updated_at_ms=max(updated_at_ms,NEW.created_at_ms)
   WHERE id=NEW.definition_id AND reservation_owner=NEW.reservation_owner
     AND reservation_fence=NEW.reservation_fence AND reserved_class_name=NEW.class_name;
  SELECT CASE WHEN changes()!=1 THEN RAISE(ABORT,'workflow reservation fence') END;
END;
CREATE TRIGGER workflow_reservation_release_terminal_worker AFTER UPDATE OF state ON worker_versions
WHEN (OLD.state IN ('staging','validating') AND NEW.state='rejected')
  OR (OLD.state IN ('ready','rejected') AND NEW.state='deleting')
BEGIN
  UPDATE workflow_definitions
     SET reserved_class_name=NULL,reservation_owner=NULL,reservation_state=NULL,
         reservation_created_definition=NULL,
         updated_at_ms=max(updated_at_ms,coalesce(NEW.rejected_at_ms,updated_at_ms))
   WHERE state IN ('creating','ready') AND reservation_owner IS NOT NULL
     AND EXISTS(SELECT 1 FROM workflow_bindings b
       WHERE b.version_id=NEW.id AND b.definition_id=workflow_definitions.id
         AND b.reservation_owner=workflow_definitions.reservation_owner
         AND b.reservation_fence=workflow_definitions.reservation_fence)
     AND NOT EXISTS(SELECT 1 FROM workflow_bindings b JOIN worker_versions v ON v.id=b.version_id
       WHERE b.definition_id=workflow_definitions.id
         AND b.reservation_owner=workflow_definitions.reservation_owner
         AND b.reservation_fence=workflow_definitions.reservation_fence
         AND v.state IN ('staging','validating','ready'))
     AND NOT EXISTS(SELECT 1 FROM workflow_versions v
       WHERE v.definition_id=workflow_definitions.id
         AND v.reservation_owner=workflow_definitions.reservation_owner
         AND v.reservation_fence=workflow_definitions.reservation_fence
         AND v.state IN ('staging','validating','ready'));
END;
CREATE TRIGGER workflow_binding_remove_ref AFTER DELETE ON workflow_bindings
BEGIN DELETE FROM workflow_referrers WHERE definition_id = OLD.definition_id AND referrer_kind = 'binding' AND referrer_id = OLD.id; END;
CREATE TRIGGER workflow_definition_current_guard BEFORE UPDATE OF current_version_id,state ON workflow_definitions
WHEN NEW.current_version_id IS NOT NULL OR NEW.state = 'ready'
BEGIN
  SELECT CASE WHEN NOT EXISTS (SELECT 1 FROM workflow_versions v
    WHERE v.id = NEW.current_version_id AND v.definition_id = NEW.id AND v.state = 'ready')
    THEN RAISE(ABORT,'workflow current version is not ready') END;
END;
CREATE TRIGGER workflow_definition_delete_guard BEFORE UPDATE OF state ON workflow_definitions
WHEN NEW.state = 'tombstoned' AND (
  EXISTS (SELECT 1 FROM workflow_referrers WHERE definition_id = OLD.id) OR
  EXISTS (SELECT 1 FROM workflow_versions WHERE definition_id = OLD.id AND state IN ('staging','validating'))
) BEGIN SELECT RAISE(ABORT,'workflow is referenced'); END;
CREATE TRIGGER workflow_definition_identity_guard BEFORE UPDATE OF id,account_id,lifecycle_generation,created_at_ms
ON workflow_definitions BEGIN SELECT RAISE(ABORT,'workflow identity is immutable'); END;
CREATE TRIGGER workflow_definition_insert_guard BEFORE INSERT ON workflow_definitions
WHEN NEW.state != 'creating' OR NEW.current_version_id IS NOT NULL OR NEW.lifecycle_generation != 1
  OR NOT EXISTS(SELECT 1 FROM accounts WHERE id=NEW.account_id AND deleted_at_ms IS NULL)
BEGIN SELECT RAISE(ABORT,'workflow definition initial authority'); END;
CREATE TRIGGER workflow_definition_no_delete BEFORE DELETE ON workflow_definitions
BEGIN SELECT RAISE(ABORT,'workflow history cannot be deleted'); END;
CREATE TRIGGER workflow_definition_state_guard BEFORE UPDATE OF state ON workflow_definitions
WHEN NEW.state != OLD.state AND NOT (
  (OLD.state = 'creating' AND NEW.state IN ('ready','deleting')) OR
  (OLD.state = 'ready' AND NEW.state = 'deleting') OR
  (OLD.state = 'deleting' AND NEW.state = 'tombstoned')
) BEGIN SELECT RAISE(ABORT,'workflow state transition'); END;
CREATE TRIGGER workflow_definition_terminal_guard BEFORE UPDATE ON workflow_definitions
WHEN OLD.state = 'tombstoned' BEGIN SELECT RAISE(ABORT,'workflow tombstone is immutable'); END;
CREATE TRIGGER workflow_version_referrer_guard BEFORE DELETE ON version_referrers
WHEN (OLD.kind = 'workflow_version' AND EXISTS (SELECT 1 FROM workflow_versions
       WHERE id = OLD.ref_id AND state NOT IN ('deleting','tombstoned')))
  OR (OLD.kind = 'workflow_instance' AND EXISTS (SELECT 1 FROM workflow_instance_referrers
       WHERE instance_id = OLD.ref_id AND state != 'released'))
BEGIN SELECT RAISE(ABORT,'workflow version is referenced'); END;
CREATE TRIGGER workflow_instance_add_ref AFTER INSERT ON workflow_instance_referrers
BEGIN
  INSERT INTO version_referrers VALUES(NEW.worker_version_id,'workflow_instance',NEW.instance_id,NEW.created_at_ms);
  INSERT INTO workflow_referrers VALUES(NEW.definition_id,'instance',NEW.instance_id,NEW.created_at_ms);
END;
CREATE TRIGGER workflow_instance_generation_guard BEFORE UPDATE OF instance_generation ON workflow_instance_referrers
WHEN NOT (OLD.state='restarting' AND NEW.state='live' AND OLD.instance_generation<9223372036854775807
  AND NEW.instance_generation=OLD.instance_generation+1 AND EXISTS(
    SELECT 1 FROM workflow_instance_operations o WHERE o.instance_id=OLD.instance_id AND o.kind='restart'
      AND o.applied=1 AND o.creation_nonce=OLD.creation_nonce AND o.expected_generation=OLD.instance_generation
      AND o.target_generation=NEW.instance_generation))
BEGIN SELECT RAISE(ABORT,'workflow restart generation requires exact intent'); END;
CREATE TRIGGER workflow_instance_ref_identity_guard BEFORE UPDATE OF instance_id,definition_id,
  definition_name,external_instance_id,workflow_version_id,worker_version_id,creation_nonce,creation_operation_id,creation_batch_id,created_at_ms
ON workflow_instance_referrers BEGIN SELECT RAISE(ABORT,'workflow instance identity is immutable'); END;
CREATE TRIGGER workflow_instance_ref_insert_guard BEFORE INSERT ON workflow_instance_referrers
BEGIN
  SELECT CASE WHEN NEW.state != 'creating' OR NEW.instance_generation != 1 OR NOT EXISTS (
    SELECT 1 FROM workflow_definitions f JOIN workflow_versions v ON v.id = f.current_version_id
    WHERE f.id = NEW.definition_id AND f.state = 'ready' AND f.availability = 'healthy'
      AND v.id = NEW.workflow_version_id AND v.state = 'ready' AND v.worker_version_id = NEW.worker_version_id
  ) THEN RAISE(ABORT,'workflow creation authority') END;
END;
CREATE TRIGGER workflow_instance_ref_state_guard BEFORE UPDATE OF state ON workflow_instance_referrers
WHEN NEW.state != OLD.state AND NOT (
  (OLD.state='creating' AND NEW.state='live') OR
  (OLD.state='live' AND NEW.state='retained') OR
  (OLD.state IN ('live','retained') AND NEW.state='restarting' AND EXISTS(
    SELECT 1 FROM workflow_instance_operations o WHERE o.instance_id=OLD.instance_id AND o.kind='restart'
      AND o.creation_nonce=OLD.creation_nonce AND o.expected_generation=OLD.instance_generation AND o.prior_ref_state=OLD.state AND o.applied=0)) OR
  (OLD.state='restarting' AND EXISTS(SELECT 1 FROM workflow_instance_operations o WHERE o.instance_id=OLD.instance_id
    AND o.kind='restart' AND o.creation_nonce=OLD.creation_nonce AND o.expected_generation=OLD.instance_generation
    AND ((o.applied=0 AND NEW.state=o.prior_ref_state AND NEW.instance_generation=o.expected_generation)
      OR (o.applied=1 AND NEW.state='live' AND NEW.instance_generation=o.target_generation)))) OR
  (OLD.state='retained' AND NEW.state='releasing' AND EXISTS(SELECT 1 FROM workflow_instance_operations o
    WHERE o.instance_id=OLD.instance_id AND o.kind='purge' AND o.applied=1 AND o.creation_nonce=OLD.creation_nonce
      AND o.expected_generation=OLD.instance_generation)) OR
  (OLD.state='releasing' AND NEW.state='released')
) BEGIN SELECT RAISE(ABORT,'workflow referrer transition'); END;
CREATE TRIGGER workflow_instance_ref_terminal_guard BEFORE UPDATE ON workflow_instance_referrers
WHEN OLD.state = 'released' BEGIN SELECT RAISE(ABORT,'workflow released history is immutable'); END;
CREATE TRIGGER workflow_instance_release_ref AFTER UPDATE OF state ON workflow_instance_referrers
WHEN NEW.state = 'released'
BEGIN
  DELETE FROM version_referrers WHERE version_id = NEW.worker_version_id AND kind = 'workflow_instance' AND ref_id = NEW.instance_id;
  DELETE FROM workflow_referrers WHERE definition_id = NEW.definition_id AND referrer_kind = 'instance' AND referrer_id = NEW.instance_id;
END;
CREATE TRIGGER workflow_instance_reservation_delete_guard BEFORE DELETE ON workflow_instance_referrers
WHEN OLD.state!='creating' AND NOT (OLD.state='released' AND EXISTS(
  SELECT 1 FROM workflow_instance_operations o WHERE o.instance_id=OLD.instance_id AND o.kind='purge'
    AND o.applied=1 AND o.creation_nonce=OLD.creation_nonce AND o.expected_generation=OLD.instance_generation))
BEGIN SELECT RAISE(ABORT,'workflow history requires a proven purge'); END;
CREATE TRIGGER workflow_instance_reservation_remove_ref AFTER DELETE ON workflow_instance_referrers
BEGIN
  DELETE FROM version_referrers WHERE version_id = OLD.worker_version_id AND kind = 'workflow_instance' AND ref_id = OLD.instance_id;
  DELETE FROM workflow_referrers WHERE definition_id = OLD.definition_id AND referrer_kind = 'instance' AND referrer_id = OLD.instance_id;
END;
CREATE TRIGGER workflow_operation_apply_guard BEFORE UPDATE OF applied ON workflow_instance_operations
WHEN OLD.applied!=0 OR NEW.applied!=1
BEGIN SELECT RAISE(ABORT,'workflow operation proof is monotonic'); END;
CREATE TRIGGER workflow_operation_delete_guard BEFORE DELETE ON workflow_instance_operations
WHEN NOT (
  (OLD.applied=0 AND EXISTS(SELECT 1 FROM workflow_instance_referrers r WHERE r.instance_id=OLD.instance_id
    AND r.instance_generation=OLD.expected_generation AND r.state=OLD.prior_ref_state)) OR
  (OLD.kind='restart' AND OLD.applied=1 AND EXISTS(SELECT 1 FROM workflow_instance_referrers r
    WHERE r.instance_id=OLD.instance_id AND r.instance_generation=OLD.target_generation AND r.state='live')) OR
  (OLD.kind='purge' AND OLD.applied=1 AND NOT EXISTS(SELECT 1 FROM workflow_instance_referrers WHERE instance_id=OLD.instance_id))
) BEGIN SELECT RAISE(ABORT,'workflow operation is unfinished'); END;
CREATE TRIGGER workflow_operation_identity_guard BEFORE UPDATE OF operation_id,instance_id,creation_nonce,
  expected_generation,target_generation,kind,restart_from_name,restart_from_count,restart_from_kind,
  prior_ref_state,created_at_ms ON workflow_instance_operations
BEGIN SELECT RAISE(ABORT,'workflow operation is immutable'); END;
CREATE TRIGGER workflow_operation_insert_guard BEFORE INSERT ON workflow_instance_operations
WHEN NEW.applied!=0 OR NOT EXISTS(
  SELECT 1 FROM workflow_instance_referrers r JOIN workflow_versions v ON v.id=r.workflow_version_id
  WHERE r.instance_id=NEW.instance_id AND r.creation_nonce=NEW.creation_nonce
    AND r.instance_generation=NEW.expected_generation AND r.state=NEW.prior_ref_state
    AND v.capability_version=1 AND (NEW.kind='purge' OR (v.state='ready' AND EXISTS(
      SELECT 1 FROM workflow_definitions f JOIN worker_versions d ON d.id=r.worker_version_id
      JOIN workers w ON w.id=d.worker_id WHERE f.id=r.definition_id AND f.state='ready' AND f.availability='healthy'
        AND d.state='ready' AND w.deleted_at_ms IS NULL))))
BEGIN SELECT RAISE(ABORT,'workflow operation identity'); END;
CREATE TRIGGER workflow_operation_sequence_immutable BEFORE UPDATE OF operation_sequence ON workflow_instance_operations
BEGIN SELECT RAISE(ABORT,'workflow operation sequence is immutable'); END;
CREATE TRIGGER workflow_operation_sequence_insert_guard BEFORE INSERT ON workflow_instance_operations
WHEN NOT EXISTS(SELECT 1 FROM workflow_instance_referrers r WHERE r.instance_id=NEW.instance_id
  AND r.operation_sequence=NEW.operation_sequence AND r.operation_sequence>=1)
BEGIN SELECT RAISE(ABORT,'workflow operation sequence does not match its reservation'); END;
CREATE TRIGGER workflow_operation_sequence_reservation_guard BEFORE UPDATE OF operation_sequence ON workflow_instance_referrers
WHEN NEW.operation_sequence!=OLD.operation_sequence+1 OR OLD.operation_sequence=9223372036854775807
  OR OLD.state NOT IN ('live','retained') OR EXISTS(SELECT 1 FROM workflow_instance_operations WHERE instance_id=OLD.instance_id)
BEGIN SELECT RAISE(ABORT,'workflow operation sequence requires a free intent slot'); END;
CREATE TRIGGER workflow_queue_conflict BEFORE INSERT ON queue_producer_bindings
WHEN EXISTS(SELECT 1 FROM workflow_bindings WHERE version_id = NEW.version_id AND name = NEW.name)
BEGIN SELECT RAISE(ABORT,'workflow queue name conflict'); END;
CREATE TRIGGER workflow_referrer_guard BEFORE DELETE ON workflow_referrers
WHEN (OLD.referrer_kind = 'binding' AND EXISTS (
      SELECT 1 FROM workflow_bindings b
      JOIN worker_versions d ON d.id=b.version_id
      JOIN workers w ON w.id=d.worker_id
      WHERE b.id=OLD.referrer_id AND w.deleted_at_ms IS NULL
    ))
  OR (OLD.referrer_kind = 'instance' AND EXISTS (SELECT 1 FROM workflow_instance_referrers
      WHERE instance_id = OLD.referrer_id AND state != 'released'))
BEGIN SELECT RAISE(ABORT,'workflow is referenced'); END;
CREATE TRIGGER workflow_resource_conflict BEFORE INSERT ON version_bindings
WHEN EXISTS(SELECT 1 FROM workflow_bindings WHERE version_id = NEW.version_id AND name = NEW.name)
BEGIN SELECT RAISE(ABORT,'workflow resource name conflict'); END;
CREATE TRIGGER workflow_secret_conflict BEFORE INSERT ON version_secrets
WHEN EXISTS(SELECT 1 FROM workflow_bindings WHERE version_id = NEW.version_id AND name = NEW.name)
BEGIN SELECT RAISE(ABORT,'workflow secret name conflict'); END;
CREATE TRIGGER workflow_var_conflict BEFORE INSERT ON version_vars
WHEN EXISTS(SELECT 1 FROM workflow_bindings WHERE version_id = NEW.version_id AND name = NEW.name)
BEGIN SELECT RAISE(ABORT,'workflow variable name conflict'); END;
CREATE TRIGGER workflow_version_add_ref AFTER INSERT ON workflow_versions
BEGIN INSERT INTO version_referrers VALUES(NEW.worker_version_id,'workflow_version',NEW.id,NEW.created_at_ms); END;
CREATE TRIGGER workflow_version_delete_guard BEFORE UPDATE OF state ON workflow_versions
WHEN NEW.state IN ('deleting','tombstoned') AND (
  EXISTS (SELECT 1 FROM workflow_definitions WHERE current_version_id = OLD.id) OR
  EXISTS (SELECT 1 FROM workflow_instance_referrers WHERE workflow_version_id = OLD.id AND state != 'released')
) BEGIN SELECT RAISE(ABORT,'workflow version is referenced'); END;
CREATE TRIGGER workflow_version_identity_guard BEFORE UPDATE OF id,definition_id,version_number,worker_id,
  worker_version_id,class_name,worker_code_sha256,loader_schema_version,capability_version,descriptor_sha256,created_at_ms
ON workflow_versions BEGIN SELECT RAISE(ABORT,'workflow frozen version is immutable'); END;
CREATE TRIGGER workflow_version_insert_guard BEFORE INSERT ON workflow_versions
BEGIN
  SELECT CASE WHEN NEW.state != 'staging' OR NOT EXISTS (
    SELECT 1 FROM workflow_definitions f JOIN workers w ON w.account_id = f.account_id
    JOIN worker_versions d ON d.worker_id = w.id
    WHERE f.id = NEW.definition_id AND f.state IN ('creating','ready')
      AND w.id = NEW.worker_id AND w.deleted_at_ms IS NULL
      AND d.id = NEW.worker_version_id AND d.state = 'ready'
      AND d.worker_code_sha256 = NEW.worker_code_sha256 AND d.loader_schema_version = NEW.loader_schema_version
  ) THEN RAISE(ABORT,'workflow version authority') END;
END;
CREATE TRIGGER workflow_version_no_delete BEFORE DELETE ON workflow_versions
BEGIN SELECT RAISE(ABORT,'workflow version history cannot be deleted'); END;
CREATE TRIGGER workflow_version_release_ref AFTER UPDATE OF state ON workflow_versions
WHEN NEW.state = 'deleting'
BEGIN DELETE FROM version_referrers WHERE version_id = OLD.worker_version_id AND kind = 'workflow_version' AND ref_id = OLD.id; END;
CREATE TRIGGER workflow_version_state_guard BEFORE UPDATE OF state ON workflow_versions
WHEN NEW.state != OLD.state AND NOT (
  (OLD.state = 'staging' AND NEW.state IN ('validating','rejected')) OR
  (OLD.state = 'validating' AND NEW.state IN ('ready','rejected')) OR
  (OLD.state IN ('ready','rejected') AND NEW.state = 'deleting') OR
  (OLD.state = 'deleting' AND NEW.state = 'tombstoned')
) BEGIN SELECT RAISE(ABORT,'workflow version transition'); END;
CREATE TRIGGER workflow_version_terminal_guard BEFORE UPDATE ON workflow_versions
WHEN OLD.state = 'tombstoned' BEGIN SELECT RAISE(ABORT,'workflow version tombstone is immutable'); END;
CREATE TABLE version_assets (
  version_id TEXT PRIMARY KEY REFERENCES worker_versions(id),
  manifest_sha256 BLOB NOT NULL CHECK(length(manifest_sha256) = 32),
  manifest_size INTEGER NOT NULL CHECK(manifest_size > 0),
  manifest_schema_version INTEGER NOT NULL CHECK(manifest_schema_version = 1),
  manifest_json BLOB NOT NULL,
  routing_config_json BLOB NOT NULL,
  binding_name TEXT,
  logical_file_count INTEGER NOT NULL CHECK(logical_file_count > 0),
  logical_total_bytes INTEGER NOT NULL CHECK(logical_total_bytes >= 0),
  created_at_ms INTEGER NOT NULL,
  CHECK(binding_name IS NULL OR length(binding_name) BETWEEN 1 AND 64)
) WITHOUT ROWID, STRICT;
CREATE TABLE version_object_refs (
  version_id TEXT NOT NULL REFERENCES worker_versions(id),
  object_kind TEXT NOT NULL CHECK(object_kind IN ('bundle', 'asset_manifest', 'asset_blob')),
  sha256 BLOB NOT NULL CHECK(length(sha256) = 32),
  size INTEGER NOT NULL CHECK(size >= 0),
  created_at_ms INTEGER NOT NULL,
  PRIMARY KEY(version_id, object_kind, sha256)
) WITHOUT ROWID, STRICT;
CREATE INDEX version_object_refs_digest
ON version_object_refs(sha256, version_id);
CREATE TABLE version_uploads (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  worker_id TEXT NOT NULL REFERENCES workers(id),
  idempotency_key TEXT NOT NULL,
  input_fingerprint BLOB NOT NULL CHECK(length(input_fingerprint) = 32),
  content_kind TEXT NOT NULL CHECK(content_kind IN ('worker', 'assets_only')),
  bundle_sha256 BLOB CHECK(bundle_sha256 IS NULL OR length(bundle_sha256) = 32),
  bundle_size INTEGER CHECK(bundle_size IS NULL OR bundle_size >= 0),
  manifest_sha256 BLOB NOT NULL CHECK(length(manifest_sha256) = 32),
  manifest_size INTEGER NOT NULL CHECK(manifest_size > 0),
  manifest_json BLOB NOT NULL,
  routing_config_json BLOB NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('open', 'finalizing', 'committed', 'aborted', 'expired')),
  version_id TEXT,
  finalize_fingerprint BLOB CHECK(finalize_fingerprint IS NULL OR length(finalize_fingerprint) = 32),
  finalize_owner_startup_id TEXT,
  finalize_response_json BLOB,
  finalize_error_code TEXT,
  created_at_ms INTEGER NOT NULL,
  expires_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  CHECK(
    (content_kind = 'worker' AND bundle_sha256 IS NOT NULL AND bundle_size IS NOT NULL) OR
    (content_kind = 'assets_only' AND bundle_sha256 IS NULL AND bundle_size IS NULL)
  ),
  CHECK(
    (status IN ('open', 'aborted', 'expired') AND version_id IS NULL
      AND finalize_fingerprint IS NULL AND finalize_owner_startup_id IS NULL
      AND finalize_response_json IS NULL AND finalize_error_code IS NULL) OR
    (status = 'finalizing' AND version_id IS NOT NULL
      AND finalize_fingerprint IS NOT NULL AND finalize_owner_startup_id IS NOT NULL
      AND finalize_response_json IS NULL AND finalize_error_code IS NULL) OR
    (status = 'committed' AND version_id IS NOT NULL
      AND finalize_fingerprint IS NOT NULL AND finalize_owner_startup_id IS NOT NULL
      AND ((finalize_response_json IS NOT NULL AND finalize_error_code IS NULL) OR
           (finalize_response_json IS NULL AND finalize_error_code IS NOT NULL)))
  ),
  UNIQUE(account_id, worker_id, idempotency_key)
) STRICT;
CREATE INDEX version_uploads_worker_status
ON version_uploads(account_id, worker_id, status, expires_at_ms);
CREATE TABLE asset_upload_sessions (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  script_name TEXT NOT NULL CHECK(length(script_name) BETWEEN 1 AND 63),
  status TEXT NOT NULL CHECK(status IN ('open', 'complete', 'reserved', 'consumed', 'expired')),
  reservation_id TEXT,
  released_reservation_id TEXT,
  created_at_ms INTEGER NOT NULL,
  expires_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  CHECK(
    (status IN ('open', 'expired') AND reservation_id IS NULL AND released_reservation_id IS NULL) OR
    (status = 'complete' AND reservation_id IS NULL) OR
    (status IN ('reserved', 'consumed') AND reservation_id IS NOT NULL AND released_reservation_id IS NULL)
  )
) STRICT;
CREATE TRIGGER asset_upload_session_identity_immutable
BEFORE UPDATE ON asset_upload_sessions
WHEN NEW.id != OLD.id OR NEW.account_id != OLD.account_id
  OR NEW.script_name != OLD.script_name OR NEW.created_at_ms != OLD.created_at_ms
  OR NEW.expires_at_ms != OLD.expires_at_ms
BEGIN
  SELECT RAISE(ABORT, 'asset upload session identity is immutable');
END;
CREATE TRIGGER asset_upload_session_transition_guard
BEFORE UPDATE ON asset_upload_sessions
WHEN NOT (
  (OLD.status = 'open' AND NEW.status IN ('open', 'complete', 'expired')) OR
  (OLD.status = 'complete' AND NEW.status IN ('complete', 'reserved')) OR
  (OLD.status = 'reserved' AND NEW.status IN ('reserved', 'complete', 'consumed')) OR
  (OLD.status = 'consumed' AND NEW.status = 'consumed') OR
  (OLD.status = 'expired' AND NEW.status = 'expired')
)
BEGIN
  SELECT RAISE(ABORT, 'invalid asset upload session transition');
END;
CREATE INDEX asset_upload_sessions_scope
ON asset_upload_sessions(account_id, script_name, status, expires_at_ms);
CREATE TABLE asset_upload_entries (
  session_id TEXT NOT NULL REFERENCES asset_upload_sessions(id),
  path TEXT NOT NULL,
  wrangler_hash TEXT NOT NULL CHECK(length(wrangler_hash) = 32),
  size INTEGER NOT NULL CHECK(size >= 0),
  content_type TEXT,
  artifact_sha256 BLOB CHECK(artifact_sha256 IS NULL OR length(artifact_sha256) = 32),
  uploaded_at_ms INTEGER,
  PRIMARY KEY(session_id, path),
  CHECK((artifact_sha256 IS NULL AND uploaded_at_ms IS NULL) OR
        (artifact_sha256 IS NOT NULL AND uploaded_at_ms IS NOT NULL))
) WITHOUT ROWID, STRICT;
CREATE TRIGGER asset_upload_entry_evidence_immutable
BEFORE UPDATE ON asset_upload_entries
WHEN NEW.session_id != OLD.session_id OR NEW.path != OLD.path
  OR NEW.wrangler_hash != OLD.wrangler_hash OR NEW.size != OLD.size
  OR (OLD.artifact_sha256 IS NOT NULL AND (
    NEW.artifact_sha256 IS NOT OLD.artifact_sha256
    OR NEW.content_type IS NOT OLD.content_type
    OR NEW.uploaded_at_ms IS NOT OLD.uploaded_at_ms
  ))
  OR (OLD.artifact_sha256 IS NULL AND NEW.artifact_sha256 IS NOT NULL AND (
    length(NEW.artifact_sha256) != 32 OR NEW.content_type IS NULL OR NEW.uploaded_at_ms IS NULL
  ))
BEGIN
  SELECT RAISE(ABORT, 'asset upload entry evidence is immutable');
END;
CREATE INDEX asset_upload_entries_hash
ON asset_upload_entries(session_id, wrangler_hash);
CREATE TABLE version_upload_objects (
  session_id TEXT NOT NULL REFERENCES version_uploads(id),
  sha256 BLOB NOT NULL CHECK(length(sha256) = 32),
  object_kind TEXT NOT NULL CHECK(object_kind IN ('bundle', 'asset_manifest', 'asset_blob')),
  size INTEGER NOT NULL CHECK(size >= 0),
  verified INTEGER NOT NULL DEFAULT 0 CHECK(verified IN (0, 1)),
  verified_at_ms INTEGER,
  PRIMARY KEY(session_id, sha256)
) WITHOUT ROWID, STRICT;
CREATE TRIGGER version_assets_insert_guard
BEFORE INSERT ON version_assets
WHEN (SELECT state FROM worker_versions WHERE id = NEW.version_id) != 'staging'
BEGIN
  SELECT RAISE(ABORT, 'immutable version assets');
END;
CREATE TRIGGER version_assets_update_guard
BEFORE UPDATE ON version_assets
BEGIN
  SELECT RAISE(ABORT, 'immutable version assets');
END;
CREATE TRIGGER version_assets_delete_guard
BEFORE DELETE ON version_assets
WHEN (SELECT state FROM worker_versions WHERE id = OLD.version_id) != 'deleting'
BEGIN
  SELECT RAISE(ABORT, 'immutable version assets');
END;
CREATE TRIGGER version_object_refs_insert_guard
BEFORE INSERT ON version_object_refs
WHEN (SELECT state FROM worker_versions WHERE id = NEW.version_id) != 'staging'
BEGIN
  SELECT RAISE(ABORT, 'immutable version object refs');
END;
CREATE TRIGGER version_object_refs_update_guard
BEFORE UPDATE ON version_object_refs
BEGIN
  SELECT RAISE(ABORT, 'immutable version object refs');
END;
CREATE TRIGGER version_object_refs_delete_guard
BEFORE DELETE ON version_object_refs
WHEN (SELECT state FROM worker_versions WHERE id = OLD.version_id) != 'deleting'
BEGIN
  SELECT RAISE(ABORT, 'immutable version object refs');
END;
CREATE TABLE version_services (
  version_id TEXT NOT NULL REFERENCES worker_versions(id),
  binding_name TEXT NOT NULL,
  target_worker_id TEXT NOT NULL REFERENCES workers(id),
  entrypoint TEXT,
  props_json BLOB CHECK(props_json IS NULL OR length(props_json) BETWEEN 2 AND 65536),
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms INTEGER NOT NULL,
  PRIMARY KEY(version_id, binding_name),
  CHECK(length(binding_name) BETWEEN 1 AND 64),
  CHECK(entrypoint IS NULL OR length(entrypoint) BETWEEN 1 AND 128)
) WITHOUT ROWID, STRICT;
CREATE INDEX version_services_target
ON version_services(target_worker_id, version_id);
CREATE TRIGGER version_services_insert_guard
BEFORE INSERT ON version_services
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1
    FROM worker_versions d
    JOIN workers caller ON caller.id = d.worker_id
    JOIN workers target ON target.id = NEW.target_worker_id
    WHERE d.id = NEW.version_id
      AND d.state = 'staging'
      AND caller.deleted_at_ms IS NULL
      AND target.deleted_at_ms IS NULL
      AND caller.account_id = target.account_id
  ) THEN RAISE(ABORT, 'service binding authority invariant') END;
  SELECT CASE WHEN NEW.binding_name GLOB '*[^A-Za-z0-9_$]*'
    OR NEW.binding_name GLOB '[^A-Za-z_$]*'
    OR NEW.binding_name GLOB 'OPEN_COMPUTE_*'
    OR NEW.binding_name GLOB '__*'
  THEN RAISE(ABORT, 'service binding name invariant') END;
  SELECT CASE WHEN NEW.entrypoint IS NOT NULL AND (
    NEW.entrypoint GLOB '*[^A-Za-z0-9_$]*'
    OR NEW.entrypoint GLOB '[^A-Za-z_$]*'
  ) THEN RAISE(ABORT, 'service entrypoint invariant') END;
  SELECT CASE WHEN EXISTS (
    SELECT 1 FROM version_vars
    WHERE version_id = NEW.version_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM version_secrets
    WHERE version_id = NEW.version_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM version_bindings
    WHERE version_id = NEW.version_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM queue_producer_bindings
    WHERE version_id = NEW.version_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM workflow_bindings
    WHERE version_id = NEW.version_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM version_assets
    WHERE version_id = NEW.version_id AND binding_name = NEW.binding_name
  ) THEN RAISE(ABORT, 'service env name conflict') END;
END;
CREATE TRIGGER version_services_update_guard
BEFORE UPDATE ON version_services
BEGIN
  SELECT RAISE(ABORT, 'immutable version service');
END;
CREATE TRIGGER version_services_delete_guard
BEFORE DELETE ON version_services
WHEN (SELECT state FROM worker_versions WHERE id = OLD.version_id)
  NOT IN ('staging', 'rejected', 'deleting')
BEGIN
  SELECT RAISE(ABORT, 'immutable version service');
END;
CREATE TRIGGER version_vars_service_name_guard
BEFORE INSERT ON version_vars
WHEN EXISTS (
  SELECT 1 FROM version_services
  WHERE version_id = NEW.version_id AND binding_name = NEW.name
)
BEGIN
  SELECT RAISE(ABORT, 'service env name conflict');
END;
CREATE TRIGGER version_secrets_service_name_guard
BEFORE INSERT ON version_secrets
WHEN EXISTS (
  SELECT 1 FROM version_services
  WHERE version_id = NEW.version_id AND binding_name = NEW.name
)
BEGIN
  SELECT RAISE(ABORT, 'service env name conflict');
END;
CREATE TRIGGER version_bindings_service_name_guard
BEFORE INSERT ON version_bindings
WHEN EXISTS (
  SELECT 1 FROM version_services
  WHERE version_id = NEW.version_id AND binding_name = NEW.name
)
BEGIN
  SELECT RAISE(ABORT, 'service env name conflict');
END;
CREATE TRIGGER queue_producer_bindings_service_name_guard
BEFORE INSERT ON queue_producer_bindings
WHEN EXISTS (
  SELECT 1 FROM version_services
  WHERE version_id = NEW.version_id AND binding_name = NEW.name
)
BEGIN
  SELECT RAISE(ABORT, 'service env name conflict');
END;
CREATE TRIGGER workflow_bindings_service_name_guard
BEFORE INSERT ON workflow_bindings
WHEN EXISTS (
  SELECT 1 FROM version_services
  WHERE version_id = NEW.version_id AND binding_name = NEW.name
)
BEGIN
  SELECT RAISE(ABORT, 'service env name conflict');
END;
CREATE TRIGGER version_assets_service_name_guard
BEFORE INSERT ON version_assets
WHEN NEW.binding_name IS NOT NULL AND EXISTS (
  SELECT 1 FROM version_services
  WHERE version_id = NEW.version_id AND binding_name = NEW.binding_name
)
BEGIN
  SELECT RAISE(ABORT, 'service env name conflict');
END;
CREATE TABLE version_cache_policies (
  version_id TEXT NOT NULL REFERENCES worker_versions(id),
  entrypoint_name TEXT NOT NULL,
  enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
  cross_version_cache INTEGER NOT NULL CHECK(cross_version_cache IN (0, 1)),
  PRIMARY KEY(version_id, entrypoint_name),
  CHECK(length(entrypoint_name) <= 128)
) WITHOUT ROWID, STRICT;
CREATE TABLE version_builtin_bindings (
  version_id TEXT NOT NULL REFERENCES worker_versions(id),
  binding_name TEXT NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN (
    'worker_loader', 'ai', 'images', 'version_metadata', 'wasm_module', 'text_blob', 'data_blob'
  )),
  tag TEXT,
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  PRIMARY KEY(version_id, binding_name),
  CHECK(length(binding_name) BETWEEN 1 AND 64),
  CHECK(tag IS NULL OR length(tag) BETWEEN 1 AND 1024),
  CHECK(
    (kind IN ('worker_loader', 'ai', 'images') AND tag IS NULL)
    OR (kind IN ('version_metadata', 'wasm_module', 'text_blob', 'data_blob') AND tag IS NOT NULL)
  )
) WITHOUT ROWID, STRICT;
CREATE UNIQUE INDEX version_builtin_bindings_singleton_kind
ON version_builtin_bindings(version_id, kind)
WHERE kind IN ('ai', 'images', 'version_metadata');
CREATE TRIGGER version_cache_policies_insert_guard
BEFORE INSERT ON version_cache_policies
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM worker_versions
    WHERE id = NEW.version_id AND state = 'staging' AND content_kind = 'worker'
  ) THEN RAISE(ABORT, 'cache policy authority invariant') END;
  SELECT CASE WHEN NEW.entrypoint_name != '' AND (
    NEW.entrypoint_name GLOB '*[^A-Za-z0-9_$]*'
    OR NEW.entrypoint_name GLOB '[^A-Za-z_$]*'
  ) THEN RAISE(ABORT, 'cache entrypoint invariant') END;
END;
CREATE TRIGGER version_cache_policies_update_guard
BEFORE UPDATE ON version_cache_policies
BEGIN
  SELECT RAISE(ABORT, 'immutable version cache policy');
END;
CREATE TRIGGER version_cache_policies_delete_guard
BEFORE DELETE ON version_cache_policies
WHEN (SELECT state FROM worker_versions WHERE id = OLD.version_id)
  NOT IN ('staging', 'rejected', 'deleting')
BEGIN
  SELECT RAISE(ABORT, 'immutable version cache policy');
END;
CREATE TRIGGER version_builtin_bindings_insert_guard
BEFORE INSERT ON version_builtin_bindings
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM worker_versions
    WHERE id = NEW.version_id AND state = 'staging' AND content_kind = 'worker'
  ) THEN RAISE(ABORT, 'builtin binding authority invariant') END;
  SELECT CASE WHEN NEW.binding_name GLOB '*[^A-Za-z0-9_$]*'
    OR NEW.binding_name GLOB '[^A-Za-z_$]*'
    OR NEW.binding_name GLOB 'OPEN_COMPUTE_*'
    OR NEW.binding_name GLOB '__*'
  THEN RAISE(ABORT, 'builtin binding name invariant') END;
  SELECT CASE WHEN EXISTS (
    SELECT 1 FROM version_vars WHERE version_id = NEW.version_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM version_secrets WHERE version_id = NEW.version_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM version_bindings WHERE version_id = NEW.version_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM queue_producer_bindings WHERE version_id = NEW.version_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM workflow_bindings WHERE version_id = NEW.version_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM version_services WHERE version_id = NEW.version_id AND binding_name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM version_assets WHERE version_id = NEW.version_id AND binding_name = NEW.binding_name
  ) THEN RAISE(ABORT, 'builtin binding env name conflict') END;
END;
CREATE TRIGGER version_builtin_bindings_update_guard
BEFORE UPDATE ON version_builtin_bindings
BEGIN
  SELECT RAISE(ABORT, 'immutable version builtin binding');
END;
CREATE TRIGGER version_builtin_bindings_delete_guard
BEFORE DELETE ON version_builtin_bindings
WHEN (SELECT state FROM worker_versions WHERE id = OLD.version_id)
  NOT IN ('staging', 'rejected', 'deleting')
BEGIN
  SELECT RAISE(ABORT, 'immutable version builtin binding');
END;
CREATE TABLE vectorize_indexes (
  resource_id     TEXT PRIMARY KEY REFERENCES resources(id),
  storage_key     TEXT NOT NULL UNIQUE,
  schema_version  INTEGER NOT NULL CHECK(schema_version = 1),
  dimensions      INTEGER NOT NULL CHECK(dimensions BETWEEN 1 AND 1536),
  metric          TEXT NOT NULL CHECK(metric IN ('cosine', 'euclidean', 'dot-product')),
  description     TEXT,
  quota_vectors   INTEGER NOT NULL CHECK(quota_vectors BETWEEN 1 AND 200000),
  quota_bytes     INTEGER NOT NULL CHECK(quota_bytes >= 1048576),
  created_at_ms   INTEGER NOT NULL
) STRICT;
CREATE TRIGGER vectorize_index_insert_guard
BEFORE INSERT ON vectorize_indexes
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM resources
    WHERE id = NEW.resource_id
      AND kind = 'vectorize_index'
      AND state = 'creating'
      AND driver_schema_version = NEW.schema_version
      AND created_at_ms = NEW.created_at_ms
  ) THEN RAISE(ABORT, 'vectorize index authority invariant') END;
END;
CREATE TRIGGER vectorize_index_identity_immutable_guard
BEFORE UPDATE ON vectorize_indexes
BEGIN
  SELECT RAISE(ABORT, 'immutable vectorize index identity');
END;
CREATE TRIGGER vectorize_index_delete_guard
BEFORE DELETE ON vectorize_indexes
WHEN (SELECT state FROM resources WHERE id = OLD.resource_id)
  NOT IN ('deleting', 'tombstoned')
BEGIN
  SELECT RAISE(ABORT, 'live vectorize index locator');
END;
CREATE TRIGGER vectorize_resource_tombstone_retire_locator
AFTER UPDATE OF state ON resources
WHEN NEW.state = 'tombstoned' AND NEW.kind = 'vectorize_index'
BEGIN
  DELETE FROM vectorize_indexes WHERE resource_id = NEW.id;
END;
CREATE TABLE ai_search_namespaces (
  resource_id    TEXT PRIMARY KEY REFERENCES resources(id),
  description    TEXT CHECK(description IS NULL OR length(description) <= 256),
  created_at_ms  INTEGER NOT NULL
) STRICT;
CREATE TRIGGER ai_search_namespace_insert_guard
BEFORE INSERT ON ai_search_namespaces
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM resources
    WHERE id = NEW.resource_id
      AND kind = 'ai_search_namespace'
      AND state = 'creating'
      AND driver_schema_version = 1
      AND created_at_ms = NEW.created_at_ms
  ) THEN RAISE(ABORT, 'AI Search namespace authority invariant') END;
END;
CREATE TRIGGER ai_search_namespace_identity_immutable_guard
BEFORE UPDATE OF resource_id, created_at_ms ON ai_search_namespaces
BEGIN
  SELECT RAISE(ABORT, 'immutable AI Search namespace identity');
END;
CREATE TRIGGER ai_search_namespace_delete_guard
BEFORE DELETE ON ai_search_namespaces
WHEN (SELECT state FROM resources WHERE id = OLD.resource_id)
  NOT IN ('deleting', 'tombstoned')
BEGIN
  SELECT RAISE(ABORT, 'live AI Search namespace locator');
END;
CREATE TRIGGER ai_search_resource_tombstone_retire_namespace
AFTER UPDATE OF state ON resources
WHEN NEW.state = 'tombstoned' AND NEW.kind = 'ai_search_namespace'
BEGIN
  DELETE FROM ai_search_namespaces WHERE resource_id = NEW.id;
END;
CREATE UNIQUE INDEX workers_live_name
ON workers(account_id, name)
WHERE deleted_at_ms IS NULL AND ownership = 'tenant';
CREATE TABLE system_owned_versions (
  kind TEXT PRIMARY KEY CHECK(kind = 'dashboard'),
  account_id TEXT NOT NULL REFERENCES accounts(id),
  worker_id TEXT NOT NULL REFERENCES workers(id),
  active_version_id TEXT REFERENCES worker_versions(id),
  assets_sha256 BLOB NOT NULL CHECK(length(assets_sha256) = 32),
  updated_at_ms INTEGER NOT NULL
) STRICT;
CREATE TABLE artifact_namespaces (
  id TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  account_id TEXT NOT NULL REFERENCES accounts(id),
  name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 64),
  jurisdiction TEXT CHECK(jurisdiction IS NULL OR length(jurisdiction) BETWEEN 1 AND 32),
  max_repositories INTEGER NOT NULL CHECK(max_repositories BETWEEN 1 AND 10000),
  max_tokens_per_repository INTEGER NOT NULL CHECK(max_tokens_per_repository BETWEEN 1 AND 1000),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  UNIQUE(account_id, name)
) STRICT;
CREATE TABLE artifact_repositories (
  id TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  namespace_id TEXT NOT NULL REFERENCES artifact_namespaces(id),
  name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 128),
  description TEXT NOT NULL DEFAULT '' CHECK(length(description) <= 2048),
  default_branch TEXT NOT NULL CHECK(length(default_branch) BETWEEN 1 AND 255),
  state TEXT NOT NULL CHECK(state IN (
    'creating', 'importing', 'forking', 'ready', 'deleting', 'failed', 'tombstoned'
  )),
  read_only INTEGER NOT NULL DEFAULT 0 CHECK(read_only IN (0, 1)),
  source TEXT,
  generation INTEGER NOT NULL DEFAULT 1 CHECK(generation >= 1),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  last_push_at_ms INTEGER,
  deleted_at_ms INTEGER,
  CHECK((state = 'tombstoned') = (deleted_at_ms IS NOT NULL))
) STRICT;
CREATE UNIQUE INDEX artifact_repositories_live_name
ON artifact_repositories(namespace_id, name)
WHERE state != 'tombstoned';
CREATE INDEX artifact_repositories_state
ON artifact_repositories(state, updated_at_ms, id);
CREATE TABLE artifact_repo_tokens (
  id TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  repository_id TEXT NOT NULL REFERENCES artifact_repositories(id),
  token_digest BLOB NOT NULL CHECK(length(token_digest) = 32),
  scope TEXT NOT NULL CHECK(scope IN ('read', 'write')),
  expires_at_ms INTEGER NOT NULL,
  created_at_ms INTEGER NOT NULL,
  revoked_at_ms INTEGER,
  UNIQUE(repository_id, token_digest)
) STRICT;
CREATE INDEX artifact_repo_tokens_active
ON artifact_repo_tokens(repository_id, expires_at_ms, id)
WHERE revoked_at_ms IS NULL;
CREATE TABLE version_artifact_bindings (
  id TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  version_id TEXT NOT NULL REFERENCES worker_versions(id),
  name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 64),
  namespace_id TEXT NOT NULL REFERENCES artifact_namespaces(id),
  namespace_generation INTEGER NOT NULL CHECK(namespace_generation >= 1),
  capability_version INTEGER NOT NULL CHECK(capability_version = 1),
  permissions_json BLOB NOT NULL,
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms INTEGER NOT NULL,
  UNIQUE(version_id, name)
) STRICT;
CREATE INDEX version_artifact_bindings_namespace
ON version_artifact_bindings(namespace_id, version_id, id);
CREATE TRIGGER artifact_namespace_identity_immutable
BEFORE UPDATE OF id, account_id, name, created_at_ms ON artifact_namespaces
BEGIN
  SELECT RAISE(ABORT, 'artifact namespace identity is immutable');
END;
CREATE TRIGGER artifact_repository_identity_immutable
BEFORE UPDATE OF id, namespace_id, name, created_at_ms ON artifact_repositories
BEGIN
  SELECT RAISE(ABORT, 'artifact repository identity is immutable');
END;
CREATE TRIGGER artifact_repository_transition_guard
BEFORE UPDATE OF state ON artifact_repositories
WHEN OLD.state != NEW.state AND NOT (
  (OLD.state IN ('creating', 'importing', 'forking') AND NEW.state IN ('ready', 'failed', 'deleting')) OR
  (OLD.state = 'ready' AND NEW.state IN ('deleting', 'failed')) OR
  (OLD.state = 'failed' AND NEW.state = 'deleting') OR
  (OLD.state = 'deleting' AND NEW.state = 'ready') OR
  (OLD.state = 'deleting' AND NEW.state = 'tombstoned')
)
BEGIN
  SELECT RAISE(ABORT, 'invalid artifact repository transition');
END;
CREATE TABLE ai_search_instances (
  resource_id            TEXT PRIMARY KEY REFERENCES resources(id),
  namespace_resource_id  TEXT NOT NULL REFERENCES ai_search_namespaces(resource_id),
  instance_key           TEXT NOT NULL CHECK(
    length(CAST(instance_key AS BLOB)) BETWEEN 1 AND 64
    AND instance_key NOT GLOB '*[' || char(0) || '-' || char(31) || ']*'
  ),
  storage_key            TEXT NOT NULL UNIQUE,
  schema_version         INTEGER NOT NULL CHECK(schema_version IN (1, 2)),
  model_contract_sha256  BLOB NOT NULL CHECK(length(model_contract_sha256) = 32),
  created_at_ms          INTEGER NOT NULL,
  UNIQUE(namespace_resource_id, instance_key)
) STRICT;
CREATE TRIGGER ai_search_instance_insert_guard
BEFORE INSERT ON ai_search_instances
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1
    FROM resources child
    JOIN resources parent
      ON parent.id = NEW.namespace_resource_id
     AND parent.account_id = child.account_id
    JOIN ai_search_namespaces namespace
      ON namespace.resource_id = parent.id
    WHERE child.id = NEW.resource_id
      AND child.kind = 'ai_search_instance'
      AND child.state = 'creating'
      AND child.driver_schema_version = NEW.schema_version
      AND child.created_at_ms = NEW.created_at_ms
      AND parent.kind = 'ai_search_namespace'
      AND parent.state = 'ready'
  ) THEN RAISE(ABORT, 'AI Search instance authority invariant') END;
END;
CREATE TRIGGER ai_search_instance_identity_immutable_guard
BEFORE UPDATE OF resource_id, namespace_resource_id, instance_key, storage_key,
                 schema_version, created_at_ms ON ai_search_instances
BEGIN
  SELECT RAISE(ABORT, 'immutable AI Search instance identity');
END;
CREATE TRIGGER ai_search_instance_referrer_insert
AFTER INSERT ON ai_search_instances
BEGIN
  INSERT INTO resource_referrers(resource_id, referrer_kind, referrer_id, created_at_ms)
  VALUES(NEW.namespace_resource_id, 'ai_search_instance', NEW.resource_id, NEW.created_at_ms);
END;
CREATE TRIGGER ai_search_instance_referrer_delete
AFTER DELETE ON ai_search_instances
BEGIN
  DELETE FROM resource_referrers
  WHERE resource_id = OLD.namespace_resource_id
    AND referrer_kind = 'ai_search_instance'
    AND referrer_id = OLD.resource_id;
END;
CREATE TRIGGER ai_search_instance_referrer_insert_guard
BEFORE INSERT ON resource_referrers
WHEN NEW.referrer_kind = 'ai_search_instance'
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM ai_search_instances
    WHERE resource_id = NEW.referrer_id
      AND namespace_resource_id = NEW.resource_id
      AND created_at_ms = NEW.created_at_ms
  ) THEN RAISE(ABORT, 'orphan AI Search instance referrer') END;
END;
CREATE TRIGGER ai_search_instance_referrer_delete_guard
BEFORE DELETE ON resource_referrers
WHEN OLD.referrer_kind = 'ai_search_instance'
 AND EXISTS (
   SELECT 1 FROM ai_search_instances child_locator
   JOIN resources child ON child.id = child_locator.resource_id
   WHERE child_locator.resource_id = OLD.referrer_id
     AND child_locator.namespace_resource_id = OLD.resource_id
     AND child.state != 'tombstoned'
 )
BEGIN
  SELECT RAISE(ABORT, 'live AI Search instance referrer');
END;
CREATE TRIGGER ai_search_instance_delete_guard
BEFORE DELETE ON ai_search_instances
WHEN (SELECT state FROM resources WHERE id = OLD.resource_id)
  NOT IN ('deleting', 'tombstoned')
BEGIN
  SELECT RAISE(ABORT, 'live AI Search instance locator');
END;
CREATE TRIGGER ai_search_namespace_child_delete_fence
BEFORE UPDATE OF state ON resources
WHEN OLD.kind = 'ai_search_namespace'
 AND NEW.state IN ('deleting', 'tombstoned')
 AND EXISTS (
   SELECT 1
   FROM ai_search_instances child_locator
   JOIN resources child ON child.id = child_locator.resource_id
   WHERE child_locator.namespace_resource_id = OLD.id
     AND child.state != 'tombstoned'
 )
BEGIN
  SELECT RAISE(ABORT, 'AI Search namespace still has live instances');
END;
CREATE TRIGGER ai_search_resource_tombstone_retire_instance
AFTER UPDATE OF state ON resources
WHEN NEW.state = 'tombstoned' AND NEW.kind = 'ai_search_instance'
BEGIN
  DELETE FROM ai_search_instances WHERE resource_id = NEW.id;
END;
CREATE TABLE ai_search_r2_sources (
  instance_resource_id  TEXT PRIMARY KEY REFERENCES ai_search_instances(resource_id) ON DELETE CASCADE,
  bucket_resource_id    TEXT NOT NULL REFERENCES r2_buckets(resource_id),
  bucket_name           TEXT NOT NULL CHECK(length(CAST(bucket_name AS BLOB)) BETWEEN 1 AND 512),
  created_at_ms          INTEGER NOT NULL
) STRICT;
CREATE INDEX ai_search_r2_sources_bucket
ON ai_search_r2_sources(bucket_resource_id, instance_resource_id);
CREATE TRIGGER ai_search_r2_source_insert_guard
BEFORE INSERT ON ai_search_r2_sources
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1
      FROM ai_search_instances instance
      JOIN resources child ON child.id = instance.resource_id
      JOIN resources bucket ON bucket.id = NEW.bucket_resource_id
      JOIN r2_buckets r2 ON r2.resource_id = bucket.id
     WHERE instance.resource_id = NEW.instance_resource_id
       AND child.kind = 'ai_search_instance'
       AND child.state = 'creating'
       AND child.account_id = bucket.account_id
       AND bucket.kind = 'r2_bucket'
       AND bucket.state = 'ready'
       AND bucket.availability = 'healthy'
       AND bucket.name = NEW.bucket_name
  ) THEN RAISE(ABORT, 'AI Search R2 source authority invariant') END;
END;
CREATE TRIGGER ai_search_r2_source_immutable_guard
BEFORE UPDATE ON ai_search_r2_sources
BEGIN
  SELECT RAISE(ABORT, 'immutable AI Search R2 source');
END;
CREATE TRIGGER ai_search_r2_source_bucket_delete_guard
BEFORE UPDATE OF state ON resources
WHEN OLD.state != 'deleting' AND NEW.state = 'deleting'
  AND EXISTS (
    SELECT 1 FROM ai_search_r2_sources
     WHERE bucket_resource_id = OLD.id
  )
BEGIN
  SELECT RAISE(ABORT, 'R2 bucket is an AI Search source');
END;
