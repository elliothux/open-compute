-- Published migrations are immutable. Collapse the old account row into the
-- one durable instance identity, rejecting ambiguous or inconsistent state.
CREATE TEMP TABLE instance_identity_guard (ok INTEGER NOT NULL CHECK (ok = 1));
INSERT INTO instance_identity_guard (ok)
SELECT CASE
  WHEN (SELECT COUNT(*) FROM accounts) = 0
    AND NOT EXISTS (SELECT 1 FROM platform_meta WHERE key IN ('instance_id', 'created_at_ms'))
    THEN 1
  WHEN (SELECT COUNT(*) FROM accounts) = 1
    AND (SELECT deleted_at_ms FROM accounts) IS NULL
    AND (SELECT id FROM accounts) =
      (SELECT CAST(value AS TEXT) FROM platform_meta WHERE key = 'instance_id')
    AND (SELECT CAST(created_at_ms AS TEXT) FROM accounts) =
      (SELECT CAST(value AS TEXT) FROM platform_meta WHERE key = 'created_at_ms')
    THEN 1
  ELSE 0
END;
DROP TABLE instance_identity_guard;

DROP TRIGGER workflow_definition_insert_guard;
DROP INDEX accounts_live_name;
ALTER TABLE accounts RENAME TO instance_identity;
ALTER TABLE instance_identity RENAME COLUMN id TO instance_id;
ALTER TABLE instance_identity DROP COLUMN name;
ALTER TABLE instance_identity DROP COLUMN deleted_at_ms;
CREATE UNIQUE INDEX instance_identity_singleton ON instance_identity ((1));
CREATE TEMP TABLE instance_row_identity_guard (ok INTEGER NOT NULL CHECK (ok = 1));
INSERT INTO instance_row_identity_guard (ok)
SELECT CASE WHEN NOT EXISTS (
  SELECT 1 FROM control_audit_events
  WHERE account_id != COALESCE((SELECT instance_id FROM instance_identity), '')
) AND NOT EXISTS (
  SELECT 1 FROM control_idempotency
  WHERE account_id != COALESCE((SELECT instance_id FROM instance_identity), '')
) AND NOT EXISTS (
  SELECT 1 FROM r2_objects
  WHERE account_id != COALESCE((SELECT instance_id FROM instance_identity), '')
) AND NOT EXISTS (
  SELECT 1 FROM r2_object_mutations
  WHERE account_id != COALESCE((SELECT instance_id FROM instance_identity), '')
) AND NOT EXISTS (
  SELECT 1 FROM r2_multipart_uploads
  WHERE account_id != COALESCE((SELECT instance_id FROM instance_identity), '')
) AND NOT EXISTS (
  SELECT 1 FROM cron_activations
  WHERE account_id != COALESCE((SELECT instance_id FROM instance_identity), '')
) AND NOT EXISTS (
  SELECT 1 FROM asset_upload_sessions
  WHERE account_id != COALESCE((SELECT instance_id FROM instance_identity), '')
) AND NOT EXISTS (
  SELECT 1 FROM version_uploads
  WHERE account_id != COALESCE((SELECT instance_id FROM instance_identity), '')
) AND NOT EXISTS (
  SELECT 1 FROM queue_consumers
  WHERE account_id != COALESCE((SELECT instance_id FROM instance_identity), '')
) AND NOT EXISTS (
  SELECT 1 FROM queues
  WHERE account_id != COALESCE((SELECT instance_id FROM instance_identity), '')
) AND NOT EXISTS (
  SELECT 1 FROM resources
  WHERE account_id != COALESCE((SELECT instance_id FROM instance_identity), '')
) AND NOT EXISTS (
  SELECT 1 FROM artifact_namespaces
  WHERE account_id != COALESCE((SELECT instance_id FROM instance_identity), '')
) AND NOT EXISTS (
  SELECT 1 FROM workers
  WHERE account_id != COALESCE((SELECT instance_id FROM instance_identity), '')
) AND NOT EXISTS (
  SELECT 1 FROM hostname_claims
  WHERE account_id != COALESCE((SELECT instance_id FROM instance_identity), '')
) AND NOT EXISTS (
  SELECT 1 FROM worker_host_routes
  WHERE account_id != COALESCE((SELECT instance_id FROM instance_identity), '')
) AND NOT EXISTS (
  SELECT 1 FROM workflow_definitions
  WHERE account_id != COALESCE((SELECT instance_id FROM instance_identity), '')
) THEN 1 ELSE 0 END;
DROP TABLE instance_row_identity_guard;
ALTER TABLE control_audit_events DROP COLUMN account_id;
ALTER TABLE r2_objects DROP COLUMN account_id;
ALTER TABLE r2_object_mutations DROP COLUMN account_id;
ALTER TABLE r2_multipart_uploads DROP COLUMN account_id;
DROP TRIGGER asset_upload_session_identity_immutable;
DROP INDEX asset_upload_sessions_scope;
ALTER TABLE asset_upload_sessions DROP COLUMN account_id;
CREATE TRIGGER asset_upload_session_identity_immutable
BEFORE UPDATE ON asset_upload_sessions
WHEN NEW.id != OLD.id OR NEW.script_name != OLD.script_name
  OR NEW.created_at_ms != OLD.created_at_ms OR NEW.expires_at_ms != OLD.expires_at_ms
BEGIN SELECT RAISE(ABORT, 'asset upload session identity is immutable'); END;
CREATE INDEX asset_upload_sessions_scope
ON asset_upload_sessions(script_name, status, expires_at_ms);
CREATE TABLE version_uploads_next (
  id TEXT PRIMARY KEY,
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
  UNIQUE(worker_id, idempotency_key)
) STRICT;
INSERT INTO version_uploads_next
  (id, worker_id, idempotency_key, input_fingerprint, content_kind, bundle_sha256,
   bundle_size, manifest_sha256, manifest_size, manifest_json, routing_config_json,
   status, version_id, finalize_fingerprint, finalize_owner_startup_id,
   finalize_response_json, finalize_error_code, created_at_ms, expires_at_ms, updated_at_ms)
SELECT id, worker_id, idempotency_key, input_fingerprint, content_kind, bundle_sha256,
       bundle_size, manifest_sha256, manifest_size, manifest_json, routing_config_json,
       status, version_id, finalize_fingerprint, finalize_owner_startup_id,
       finalize_response_json, finalize_error_code, created_at_ms, expires_at_ms, updated_at_ms
FROM version_uploads;
CREATE TABLE version_upload_objects_next (
  session_id TEXT NOT NULL REFERENCES version_uploads_next(id),
  sha256 BLOB NOT NULL CHECK(length(sha256) = 32),
  object_kind TEXT NOT NULL CHECK(object_kind IN ('bundle', 'asset_manifest', 'asset_blob')),
  size INTEGER NOT NULL CHECK(size >= 0),
  verified INTEGER NOT NULL DEFAULT 0 CHECK(verified IN (0, 1)),
  verified_at_ms INTEGER,
  PRIMARY KEY(session_id, sha256)
) WITHOUT ROWID, STRICT;
INSERT INTO version_upload_objects_next
  (session_id, sha256, object_kind, size, verified, verified_at_ms)
SELECT session_id, sha256, object_kind, size, verified, verified_at_ms
FROM version_upload_objects;
DROP TABLE version_upload_objects;
DROP TABLE version_uploads;
ALTER TABLE version_uploads_next RENAME TO version_uploads;
ALTER TABLE version_upload_objects_next RENAME TO version_upload_objects;
CREATE INDEX version_uploads_worker_status
ON version_uploads(worker_id, status, expires_at_ms);
DROP TRIGGER queue_consumers_insert_guard;
DROP TRIGGER queue_consumers_identity_guard;
ALTER TABLE queue_consumers DROP COLUMN account_id;
DROP TRIGGER queue_producer_bindings_insert_guard;
DROP TRIGGER version_queue_consumers_insert_guard;
DROP TRIGGER queues_identity_update_guard;
DROP INDEX queues_live_name;
ALTER TABLE queues DROP COLUMN account_id;
CREATE UNIQUE INDEX queues_live_name ON queues(name) WHERE state != 'tombstoned';
CREATE TRIGGER queues_identity_update_guard
BEFORE UPDATE ON queues
WHEN OLD.id != NEW.id OR OLD.lifecycle_generation != NEW.lifecycle_generation OR
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
BEGIN SELECT RAISE(ABORT, 'queue immutable identity invariant'); END;
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
      AND EXISTS (SELECT 1 FROM instance_identity)
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
      AND EXISTS (SELECT 1 FROM instance_identity)
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
      AND EXISTS (SELECT 1 FROM instance_identity)
      AND q.state = 'ready' AND q.availability = 'healthy'
      AND q.lifecycle_generation = NEW.dlq_lifecycle_generation
  ) THEN RAISE(ABORT, 'queue consumer DLQ authority invariant') END;
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
    JOIN queues q ON q.id = c.queue_id
    WHERE c.id = NEW.declaration_id AND c.version_id = NEW.version_id
      AND c.queue_id = NEW.queue_id AND d.state = 'ready'
      AND d.worker_id = NEW.worker_id
      AND EXISTS (SELECT 1 FROM instance_identity)
  ) THEN RAISE(ABORT, 'queue consumer live authority invariant') END;
END;
CREATE TRIGGER queue_consumers_identity_guard
BEFORE UPDATE OF id, queue_id, created_at_ms ON queue_consumers
BEGIN SELECT RAISE(ABORT, 'queue consumer identity is immutable'); END;
DROP TRIGGER version_bindings_insert_guard;
DROP TRIGGER do_namespace_insert_guard;
DROP TRIGGER ai_search_instance_insert_guard;
DROP TRIGGER ai_search_r2_source_insert_guard;
DROP TRIGGER resource_identity_immutable_guard;
DROP INDEX resources_live_name;
ALTER TABLE resources DROP COLUMN account_id;
CREATE UNIQUE INDEX resources_live_name
ON resources(kind, name) WHERE state != 'tombstoned';
CREATE TRIGGER resource_identity_immutable_guard
BEFORE UPDATE OF id, kind, driver_schema_version, created_at_ms ON resources
BEGIN SELECT RAISE(ABORT, 'immutable resource identity'); END;
CREATE TRIGGER version_bindings_insert_guard
BEFORE INSERT ON version_bindings
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM worker_versions d
    JOIN workers w ON w.id = d.worker_id
    JOIN resources r ON r.id = NEW.resource_id
    WHERE d.id = NEW.version_id AND d.state = 'staging'
      AND EXISTS (SELECT 1 FROM instance_identity)
      AND r.state = 'ready' AND r.kind = NEW.kind
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
CREATE TRIGGER do_namespace_insert_guard
BEFORE INSERT ON do_namespaces
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM resources r
    JOIN workers w ON w.id = NEW.owner_worker_id
    WHERE r.id = NEW.resource_id AND r.kind = 'do_namespace'
      AND r.state = 'creating'
      AND EXISTS (SELECT 1 FROM instance_identity)
      AND w.deleted_at_ms IS NULL
      AND w.do_storage_id = NEW.do_storage_id
      AND r.created_at_ms = NEW.created_at_ms
  ) THEN RAISE(ABORT, 'durable object namespace authority invariant') END;
END;
CREATE TRIGGER ai_search_instance_insert_guard
BEFORE INSERT ON ai_search_instances
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM resources child
    JOIN resources parent ON parent.id = NEW.namespace_resource_id
    JOIN ai_search_namespaces namespace ON namespace.resource_id = parent.id
    WHERE child.id = NEW.resource_id
      AND child.kind = 'ai_search_instance' AND child.state = 'creating'
      AND child.driver_schema_version = NEW.schema_version
      AND child.created_at_ms = NEW.created_at_ms
      AND parent.kind = 'ai_search_namespace' AND parent.state = 'ready'
  ) THEN RAISE(ABORT, 'AI Search instance authority invariant') END;
END;
CREATE TRIGGER ai_search_r2_source_insert_guard
BEFORE INSERT ON ai_search_r2_sources
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM ai_search_instances instance
    JOIN resources child ON child.id = instance.resource_id
    JOIN resources bucket ON bucket.id = NEW.bucket_resource_id
    JOIN r2_buckets r2 ON r2.resource_id = bucket.id
    WHERE instance.resource_id = NEW.instance_resource_id
      AND child.kind = 'ai_search_instance' AND child.state = 'creating'
      AND bucket.kind = 'r2_bucket' AND bucket.state = 'ready'
      AND bucket.availability = 'healthy' AND bucket.name = NEW.bucket_name
  ) THEN RAISE(ABORT, 'AI Search R2 source authority invariant') END;
END;
CREATE TABLE artifact_namespaces_next (
  id TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  name TEXT NOT NULL UNIQUE CHECK(length(name) BETWEEN 1 AND 64),
  jurisdiction TEXT CHECK(jurisdiction IS NULL OR length(jurisdiction) BETWEEN 1 AND 32),
  max_repositories INTEGER NOT NULL CHECK(max_repositories BETWEEN 1 AND 10000),
  max_tokens_per_repository INTEGER NOT NULL CHECK(max_tokens_per_repository BETWEEN 1 AND 1000),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL
) STRICT;
INSERT INTO artifact_namespaces_next
  (id, name, jurisdiction, max_repositories, max_tokens_per_repository, created_at_ms, updated_at_ms)
SELECT id, name, jurisdiction, max_repositories, max_tokens_per_repository, created_at_ms, updated_at_ms
FROM artifact_namespaces;
CREATE TABLE artifact_repositories_next (
  id TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  namespace_id TEXT NOT NULL REFERENCES artifact_namespaces_next(id),
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
INSERT INTO artifact_repositories_next
  (id, namespace_id, name, description, default_branch, state, read_only, source,
   generation, created_at_ms, updated_at_ms, last_push_at_ms, deleted_at_ms)
SELECT id, namespace_id, name, description, default_branch, state, read_only, source,
       generation, created_at_ms, updated_at_ms, last_push_at_ms, deleted_at_ms
FROM artifact_repositories;
CREATE TABLE artifact_repo_tokens_next (
  id TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  repository_id TEXT NOT NULL REFERENCES artifact_repositories_next(id),
  token_digest BLOB NOT NULL CHECK(length(token_digest) = 32),
  scope TEXT NOT NULL CHECK(scope IN ('read', 'write')),
  expires_at_ms INTEGER NOT NULL,
  created_at_ms INTEGER NOT NULL,
  revoked_at_ms INTEGER,
  UNIQUE(repository_id, token_digest)
) STRICT;
INSERT INTO artifact_repo_tokens_next
  (id, repository_id, token_digest, scope, expires_at_ms, created_at_ms, revoked_at_ms)
SELECT id, repository_id, token_digest, scope, expires_at_ms, created_at_ms, revoked_at_ms
FROM artifact_repo_tokens;
CREATE TABLE version_artifact_bindings_next (
  id TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  version_id TEXT NOT NULL REFERENCES worker_versions(id),
  name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 64),
  namespace_id TEXT NOT NULL REFERENCES artifact_namespaces_next(id),
  namespace_generation INTEGER NOT NULL CHECK(namespace_generation >= 1),
  capability_version INTEGER NOT NULL CHECK(capability_version = 1),
  permissions_json BLOB NOT NULL,
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms INTEGER NOT NULL,
  UNIQUE(version_id, name)
) STRICT;
INSERT INTO version_artifact_bindings_next
  (id, version_id, name, namespace_id, namespace_generation, capability_version,
   permissions_json, descriptor_sha256, created_at_ms)
SELECT id, version_id, name, namespace_id, namespace_generation, capability_version,
       permissions_json, descriptor_sha256, created_at_ms
FROM version_artifact_bindings;
DROP TABLE artifact_repo_tokens;
DROP TABLE version_artifact_bindings;
DROP TABLE artifact_repositories;
DROP TABLE artifact_namespaces;
ALTER TABLE artifact_namespaces_next RENAME TO artifact_namespaces;
ALTER TABLE artifact_repositories_next RENAME TO artifact_repositories;
ALTER TABLE artifact_repo_tokens_next RENAME TO artifact_repo_tokens;
ALTER TABLE version_artifact_bindings_next RENAME TO version_artifact_bindings;
CREATE UNIQUE INDEX artifact_repositories_live_name
ON artifact_repositories(namespace_id, name) WHERE state != 'tombstoned';
CREATE INDEX artifact_repositories_state
ON artifact_repositories(state, updated_at_ms, id);
CREATE INDEX artifact_repo_tokens_active
ON artifact_repo_tokens(repository_id, expires_at_ms, id) WHERE revoked_at_ms IS NULL;
CREATE INDEX version_artifact_bindings_namespace
ON version_artifact_bindings(namespace_id, version_id, id);
CREATE TRIGGER artifact_namespace_identity_immutable
BEFORE UPDATE OF id, name, created_at_ms ON artifact_namespaces
BEGIN SELECT RAISE(ABORT, 'artifact namespace identity is immutable'); END;
CREATE TRIGGER artifact_repository_identity_immutable
BEFORE UPDATE OF id, namespace_id, name, created_at_ms ON artifact_repositories
BEGIN SELECT RAISE(ABORT, 'artifact repository identity is immutable'); END;
CREATE TRIGGER artifact_repository_transition_guard
BEFORE UPDATE OF state ON artifact_repositories
WHEN OLD.state != NEW.state AND NOT (
  (OLD.state IN ('creating', 'importing', 'forking') AND NEW.state IN ('ready', 'failed', 'deleting')) OR
  (OLD.state = 'ready' AND NEW.state IN ('deleting', 'failed')) OR
  (OLD.state = 'failed' AND NEW.state = 'deleting') OR
  (OLD.state = 'deleting' AND NEW.state = 'ready') OR
  (OLD.state = 'deleting' AND NEW.state = 'tombstoned')
)
BEGIN SELECT RAISE(ABORT, 'invalid artifact repository transition'); END;
CREATE UNIQUE INDEX hostname_claim_authority_instance
ON hostname_claims(id, namespace, exposure, state);
CREATE TABLE worker_host_routes_next (
  id TEXT PRIMARY KEY,
  claim_id TEXT NOT NULL UNIQUE,
  worker_id TEXT NOT NULL REFERENCES workers(id),
  namespace TEXT NOT NULL CHECK(namespace = 'worker'),
  exposure TEXT NOT NULL CHECK(exposure IN ('local', 'public')),
  path_prefix TEXT NOT NULL CHECK(path_prefix = '/'),
  entrypoint TEXT,
  state TEXT NOT NULL CHECK(state IN ('active', 'tombstoned')),
  generation INTEGER NOT NULL CHECK(generation > 0),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  deleted_at_ms INTEGER,
  CHECK((state = 'active' AND deleted_at_ms IS NULL) OR
        (state = 'tombstoned' AND deleted_at_ms IS NOT NULL)),
  FOREIGN KEY(claim_id, namespace, exposure, state)
    REFERENCES hostname_claims(id, namespace, exposure, state)
    DEFERRABLE INITIALLY DEFERRED
) STRICT;
INSERT INTO worker_host_routes_next
  (id, claim_id, worker_id, namespace, exposure, path_prefix, entrypoint,
   state, generation, created_at_ms, updated_at_ms, deleted_at_ms)
SELECT id, claim_id, worker_id, namespace, exposure, path_prefix, entrypoint,
       state, generation, created_at_ms, updated_at_ms, deleted_at_ms
FROM worker_host_routes;
DROP TABLE worker_host_routes;
ALTER TABLE worker_host_routes_next RENAME TO worker_host_routes;
DROP TRIGGER hostname_claim_identity_immutable;
DROP INDEX hostname_claim_authority;
ALTER TABLE hostname_claims DROP COLUMN account_id;
CREATE UNIQUE INDEX active_worker_origin
ON worker_host_routes(worker_id, exposure) WHERE state = 'active';
CREATE TRIGGER hostname_claim_identity_immutable
BEFORE UPDATE OF hostname_ascii, namespace, exposure ON hostname_claims
BEGIN SELECT RAISE(ABORT, 'hostname claim identity is immutable'); END;
CREATE TRIGGER worker_host_route_transition_guard
BEFORE UPDATE OF state ON worker_host_routes
WHEN NOT (OLD.state = 'active' AND NEW.state = 'tombstoned')
BEGIN SELECT RAISE(ABORT, 'worker host route state transition is invalid'); END;
CREATE TABLE cron_activations_next (
  id TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  worker_id TEXT NOT NULL REFERENCES workers(id),
  version_id TEXT NOT NULL REFERENCES worker_versions(id),
  expression TEXT NOT NULL CHECK(length(expression) BETWEEN 1 AND 256),
  expression_sha256 BLOB NOT NULL CHECK(length(expression_sha256) = 32),
  parser_version INTEGER NOT NULL CHECK(parser_version >= 1),
  scheduled_handler INTEGER NOT NULL CHECK(scheduled_handler IN (0, 1)),
  workflow_bindings_json BLOB NOT NULL CHECK(length(workflow_bindings_json) BETWEEN 2 AND 16384),
  activation_generation INTEGER NOT NULL CHECK(activation_generation >= 1),
  state TEXT NOT NULL CHECK(state IN ('staging', 'active', 'retiring', 'tombstoned')),
  availability TEXT NOT NULL CHECK(availability IN ('healthy', 'degraded', 'unavailable')),
  availability_code TEXT,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  deleted_at_ms INTEGER,
  UNIQUE(worker_id, activation_generation, expression),
  CHECK(availability_code IS NULL OR length(availability_code) BETWEEN 1 AND 128),
  CHECK((state = 'tombstoned') = (deleted_at_ms IS NOT NULL)),
  CHECK((availability = 'healthy') = (availability_code IS NULL))
) STRICT;
INSERT INTO cron_activations_next
  (id, worker_id, version_id, expression, expression_sha256, parser_version,
   scheduled_handler, workflow_bindings_json, activation_generation, state,
   availability, availability_code, created_at_ms, updated_at_ms, deleted_at_ms)
SELECT id, worker_id, version_id, expression, expression_sha256, parser_version,
       scheduled_handler, workflow_bindings_json, activation_generation, state,
       availability, availability_code, created_at_ms, updated_at_ms, deleted_at_ms
FROM cron_activations;
DROP TABLE cron_activations;
ALTER TABLE cron_activations_next RENAME TO cron_activations;
CREATE INDEX cron_activations_reconcile
ON cron_activations(state, availability, updated_at_ms, id)
WHERE state IN ('staging', 'retiring') OR availability != 'healthy';
CREATE TRIGGER cron_activations_insert_guard BEFORE INSERT ON cron_activations
BEGIN
  SELECT CASE WHEN NEW.state != 'staging' OR NEW.availability != 'degraded' OR
                   NEW.availability_code != 'CRON_PROJECTION_PENDING'
    THEN RAISE(ABORT, 'cron activation staging invariant') END;
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM worker_versions d
    WHERE d.id = NEW.version_id AND d.worker_id = NEW.worker_id AND d.state = 'ready'
  ) THEN RAISE(ABORT, 'cron activation authority invariant') END;
END;
CREATE TRIGGER cron_activations_identity_guard
BEFORE UPDATE OF id, worker_id, expression, expression_sha256, parser_version,
  scheduled_handler, workflow_bindings_json, activation_generation, created_at_ms
ON cron_activations
BEGIN SELECT RAISE(ABORT, 'cron activation identity is immutable'); END;
CREATE TRIGGER cron_activations_target_guard BEFORE UPDATE OF version_id ON cron_activations
BEGIN SELECT RAISE(ABORT, 'cron activation target is immutable'); END;
CREATE TRIGGER cron_activations_transition_guard BEFORE UPDATE OF state ON cron_activations
WHEN OLD.state != NEW.state AND NOT (
  (OLD.state = 'staging' AND NEW.state IN ('active', 'retiring')) OR
  (OLD.state = 'active' AND NEW.state = 'retiring') OR
  (OLD.state = 'retiring' AND NEW.state = 'tombstoned')
)
BEGIN SELECT RAISE(ABORT, 'cron activation transition invariant'); END;
CREATE TRIGGER cron_activations_tombstone_guard BEFORE UPDATE ON cron_activations
WHEN OLD.state = 'tombstoned'
BEGIN SELECT RAISE(ABORT, 'cron activation tombstone is immutable'); END;
CREATE TRIGGER cron_activations_referrer_insert AFTER INSERT ON cron_activations
BEGIN
  INSERT INTO version_referrers(version_id, kind, ref_id, created_at_ms)
  VALUES (NEW.version_id, 'cron_activation', NEW.id, NEW.created_at_ms);
END;
CREATE TRIGGER cron_activations_referrer_tombstone AFTER UPDATE OF state ON cron_activations
WHEN NEW.state = 'tombstoned'
BEGIN
  DELETE FROM version_referrers
  WHERE version_id = NEW.version_id AND kind = 'cron_activation' AND ref_id = NEW.id;
END;
DROP TRIGGER worker_version_delete_intent_guard;
DROP TRIGGER worker_deployment_delete_intent_guard;
CREATE TABLE worker_delete_intents_next (
  worker_id TEXT PRIMARY KEY REFERENCES workers(id),
  request_id TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  CHECK(length(request_id) BETWEEN 1 AND 128)
) WITHOUT ROWID, STRICT;
INSERT INTO worker_delete_intents_next(worker_id, request_id, created_at_ms)
SELECT worker_id, request_id, created_at_ms FROM worker_delete_intents;
DROP TABLE worker_delete_intents;
ALTER TABLE worker_delete_intents_next RENAME TO worker_delete_intents;
CREATE TABLE system_owned_versions_next (
  kind TEXT PRIMARY KEY CHECK(kind = 'dashboard'),
  worker_id TEXT NOT NULL REFERENCES workers(id),
  active_version_id TEXT REFERENCES worker_versions(id),
  assets_sha256 BLOB NOT NULL CHECK(length(assets_sha256) = 32),
  updated_at_ms INTEGER NOT NULL
) STRICT;
INSERT INTO system_owned_versions_next
  (kind, worker_id, active_version_id, assets_sha256, updated_at_ms)
SELECT kind, worker_id, active_version_id, assets_sha256, updated_at_ms
FROM system_owned_versions;
DROP TABLE system_owned_versions;
ALTER TABLE system_owned_versions_next RENAME TO system_owned_versions;
CREATE TRIGGER worker_version_delete_intent_guard
BEFORE INSERT ON worker_versions
WHEN EXISTS (SELECT 1 FROM worker_delete_intents i WHERE i.worker_id = NEW.worker_id)
BEGIN SELECT RAISE(ABORT, 'worker deletion in progress'); END;
CREATE TRIGGER worker_deployment_delete_intent_guard
BEFORE INSERT ON worker_deployments
WHEN EXISTS (SELECT 1 FROM worker_delete_intents i WHERE i.worker_id = NEW.worker_id)
BEGIN SELECT RAISE(ABORT, 'worker deletion in progress'); END;
CREATE TABLE control_idempotency_next (
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
  PRIMARY KEY(scope, idempotency_key)
) WITHOUT ROWID, STRICT;
INSERT INTO control_idempotency_next
  (scope, idempotency_key, fingerprint_key_id, request_fingerprint, response_json,
   version_id, resource_id, queue_id, state, created_at_ms, expires_at_ms)
SELECT scope, idempotency_key, fingerprint_key_id, request_fingerprint, response_json,
       version_id, resource_id, queue_id, state, created_at_ms, expires_at_ms
FROM control_idempotency;
DROP TABLE control_idempotency;
ALTER TABLE control_idempotency_next RENAME TO control_idempotency;
DROP TRIGGER workflow_binding_insert_guard;
DROP TRIGGER workflow_version_insert_guard;
DROP TRIGGER workflow_definition_identity_guard;
DROP INDEX workflow_definitions_live_name;
ALTER TABLE workflow_definitions DROP COLUMN account_id;
CREATE UNIQUE INDEX workflow_definitions_live_name ON workflow_definitions(name)
WHERE state != 'tombstoned';
CREATE TRIGGER workflow_definition_identity_guard
BEFORE UPDATE OF id,lifecycle_generation,created_at_ms ON workflow_definitions
BEGIN SELECT RAISE(ABORT,'workflow identity is immutable'); END;
CREATE TRIGGER workflow_definition_insert_guard BEFORE INSERT ON workflow_definitions
WHEN NEW.state != 'creating' OR NEW.current_version_id IS NOT NULL OR NEW.lifecycle_generation != 1
  OR NOT EXISTS(SELECT 1 FROM instance_identity)
BEGIN SELECT RAISE(ABORT,'workflow definition initial authority'); END;
CREATE TRIGGER workflow_binding_insert_guard BEFORE INSERT ON workflow_bindings
BEGIN
  SELECT CASE WHEN NEW.name GLOB '*[^A-Za-z0-9_$]*' OR NEW.name GLOB '[0-9]*'
    OR NEW.name GLOB 'OPEN_COMPUTE_*' OR NEW.name GLOB '__*'
    THEN RAISE(ABORT,'workflow binding name') END;
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM worker_versions d JOIN workers w ON w.id = d.worker_id
    JOIN workflow_definitions f ON f.id = NEW.definition_id
    WHERE d.id = NEW.version_id AND d.state = 'staging'
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
CREATE TRIGGER workflow_version_insert_guard BEFORE INSERT ON workflow_versions
BEGIN
  SELECT CASE WHEN NEW.state != 'staging' OR NOT EXISTS (
    SELECT 1 FROM workflow_definitions f JOIN workers w ON w.id = NEW.worker_id
    JOIN worker_versions d ON d.worker_id = w.id
    WHERE f.id = NEW.definition_id AND f.state IN ('creating','ready')
      AND w.deleted_at_ms IS NULL
      AND d.id = NEW.worker_version_id AND d.state = 'ready'
      AND d.worker_code_sha256 = NEW.worker_code_sha256 AND d.loader_schema_version = NEW.loader_schema_version
  ) THEN RAISE(ABORT,'workflow version authority') END;
END;
DROP TRIGGER version_services_insert_guard;
DROP INDEX workers_account_identity;
DROP INDEX workers_live_name;
ALTER TABLE workers DROP COLUMN account_id;
CREATE UNIQUE INDEX workers_live_name ON workers(name)
WHERE deleted_at_ms IS NULL AND ownership = 'tenant';
CREATE TRIGGER version_services_insert_guard
BEFORE INSERT ON version_services
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM worker_versions d
    JOIN workers caller ON caller.id = d.worker_id
    LEFT JOIN workers target
      ON NEW.target_kind = 'worker' AND target.id = NEW.target_worker_id
    WHERE d.id = NEW.version_id
      AND d.state = 'staging'
      AND caller.deleted_at_ms IS NULL
      AND EXISTS (SELECT 1 FROM instance_identity)
      AND (NEW.target_kind = 'extension' OR target.deleted_at_ms IS NULL)
  ) THEN RAISE(ABORT, 'service binding authority invariant') END;
  SELECT CASE WHEN NEW.binding_name GLOB '*[^A-Za-z0-9_$]*'
    OR NEW.binding_name GLOB '[^A-Za-z_$]*'
    OR NEW.binding_name GLOB 'OPEN_COMPUTE_*'
    OR NEW.binding_name GLOB '__*'
  THEN RAISE(ABORT, 'service binding name invariant') END;
  SELECT CASE WHEN NEW.target_kind = 'extension' AND (
    NEW.target_extension_name GLOB '*[^a-z0-9-]*'
    OR NEW.target_extension_name GLOB '[^a-z0-9]*'
    OR NEW.target_extension_name GLOB '*[^a-z0-9]'
  ) THEN RAISE(ABORT, 'extension service target invariant') END;
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

DELETE FROM platform_meta WHERE key IN ('instance_id', 'created_at_ms');
