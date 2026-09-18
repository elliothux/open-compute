CREATE TABLE version_services_new (
  version_id TEXT NOT NULL REFERENCES worker_versions(id),
  binding_name TEXT NOT NULL,
  target_kind TEXT NOT NULL CHECK(target_kind IN ('worker', 'extension')),
  target_worker_id TEXT REFERENCES workers(id),
  target_extension_name TEXT,
  entrypoint TEXT,
  props_json BLOB CHECK(props_json IS NULL OR length(props_json) BETWEEN 2 AND 65536),
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms INTEGER NOT NULL,
  PRIMARY KEY(version_id, binding_name),
  CHECK(length(binding_name) BETWEEN 1 AND 64),
  CHECK(entrypoint IS NULL OR length(entrypoint) BETWEEN 1 AND 128),
  CHECK(
    (target_kind = 'worker' AND target_worker_id IS NOT NULL AND target_extension_name IS NULL)
    OR
    (target_kind = 'extension' AND target_worker_id IS NULL
      AND length(target_extension_name) BETWEEN 1 AND 63)
  )
) WITHOUT ROWID, STRICT;

CREATE TRIGGER version_services_day1_break_guard
BEFORE INSERT ON version_services_new
BEGIN
  SELECT RAISE(ABORT, 'existing service descriptors require a clean data directory');
END;

INSERT INTO version_services_new (
  version_id, binding_name, target_kind, target_worker_id, target_extension_name,
  entrypoint, props_json, descriptor_sha256, created_at_ms
)
SELECT version_id, binding_name, 'worker', target_worker_id, NULL,
       entrypoint, props_json, descriptor_sha256, created_at_ms
FROM version_services
LIMIT 1;

DROP TRIGGER version_services_day1_break_guard;

DROP TRIGGER version_vars_service_name_guard;
DROP TRIGGER version_secrets_service_name_guard;
DROP TRIGGER version_bindings_service_name_guard;
DROP TRIGGER queue_producer_bindings_service_name_guard;
DROP TRIGGER workflow_bindings_service_name_guard;
DROP TRIGGER version_assets_service_name_guard;
DROP TRIGGER version_builtin_bindings_insert_guard;

DROP TABLE version_services;
ALTER TABLE version_services_new RENAME TO version_services;

CREATE INDEX version_services_target
ON version_services(target_worker_id, version_id)
WHERE target_kind = 'worker';

CREATE TRIGGER version_services_insert_guard
BEFORE INSERT ON version_services
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1
    FROM worker_versions d
    JOIN workers caller ON caller.id = d.worker_id
    LEFT JOIN workers target
      ON NEW.target_kind = 'worker' AND target.id = NEW.target_worker_id
    WHERE d.id = NEW.version_id
      AND d.state = 'staging'
      AND caller.deleted_at_ms IS NULL
      AND (NEW.target_kind = 'extension' OR (
        target.deleted_at_ms IS NULL AND caller.account_id = target.account_id
      ))
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
    SELECT 1 FROM queue_producer_bindings
    WHERE version_id = NEW.version_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM workflow_bindings WHERE version_id = NEW.version_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM version_services
    WHERE version_id = NEW.version_id AND binding_name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM version_assets
    WHERE version_id = NEW.version_id AND binding_name = NEW.binding_name
  ) THEN RAISE(ABORT, 'builtin binding env name conflict') END;
END;
