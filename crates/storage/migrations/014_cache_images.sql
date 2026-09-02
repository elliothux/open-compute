CREATE TABLE deployment_cache_policies (
  deployment_id TEXT NOT NULL REFERENCES worker_deployments(id),
  entrypoint_name TEXT NOT NULL,
  enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
  cross_version_cache INTEGER NOT NULL CHECK(cross_version_cache IN (0, 1)),
  PRIMARY KEY(deployment_id, entrypoint_name),
  CHECK(length(entrypoint_name) <= 128)
) WITHOUT ROWID, STRICT;

CREATE TABLE deployment_builtin_bindings (
  deployment_id TEXT NOT NULL REFERENCES worker_deployments(id),
  binding_name TEXT NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN ('ai', 'images', 'version_metadata')),
  tag TEXT,
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  PRIMARY KEY(deployment_id, binding_name),
  UNIQUE(deployment_id, kind),
  CHECK(length(binding_name) BETWEEN 1 AND 64),
  CHECK(tag IS NULL OR length(tag) BETWEEN 1 AND 128),
  CHECK((kind IN ('ai', 'images') AND tag IS NULL) OR kind = 'version_metadata')
) WITHOUT ROWID, STRICT;

CREATE TRIGGER deployment_cache_policies_insert_guard
BEFORE INSERT ON deployment_cache_policies
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM worker_deployments
    WHERE id = NEW.deployment_id AND state = 'staging' AND content_kind = 'worker'
  ) THEN RAISE(ABORT, 'cache policy authority invariant') END;
  SELECT CASE WHEN NEW.entrypoint_name != '' AND (
    NEW.entrypoint_name GLOB '*[^A-Za-z0-9_$]*'
    OR NEW.entrypoint_name GLOB '[^A-Za-z_$]*'
  ) THEN RAISE(ABORT, 'cache entrypoint invariant') END;
END;

CREATE TRIGGER deployment_cache_policies_update_guard
BEFORE UPDATE ON deployment_cache_policies
BEGIN
  SELECT RAISE(ABORT, 'immutable deployment cache policy');
END;

CREATE TRIGGER deployment_cache_policies_delete_guard
BEFORE DELETE ON deployment_cache_policies
WHEN (SELECT state FROM worker_deployments WHERE id = OLD.deployment_id)
  NOT IN ('staging', 'rejected', 'deleting')
BEGIN
  SELECT RAISE(ABORT, 'immutable deployment cache policy');
END;

CREATE TRIGGER deployment_builtin_bindings_insert_guard
BEFORE INSERT ON deployment_builtin_bindings
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM worker_deployments
    WHERE id = NEW.deployment_id AND state = 'staging' AND content_kind = 'worker'
  ) THEN RAISE(ABORT, 'builtin binding authority invariant') END;
  SELECT CASE WHEN NEW.binding_name GLOB '*[^A-Za-z0-9_]*'
    OR NEW.binding_name GLOB '[^A-Za-z_]*'
    OR NEW.binding_name GLOB 'OPEN_COMPUTE_*'
    OR NEW.binding_name GLOB '__*'
  THEN RAISE(ABORT, 'builtin binding name invariant') END;
  SELECT CASE WHEN EXISTS (
    SELECT 1 FROM deployment_vars WHERE deployment_id = NEW.deployment_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM deployment_secrets WHERE deployment_id = NEW.deployment_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM deployment_bindings WHERE deployment_id = NEW.deployment_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM queue_producer_bindings WHERE deployment_id = NEW.deployment_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM workflow_bindings WHERE deployment_id = NEW.deployment_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM deployment_services WHERE deployment_id = NEW.deployment_id AND binding_name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM deployment_assets WHERE deployment_id = NEW.deployment_id AND binding_name = NEW.binding_name
  ) THEN RAISE(ABORT, 'builtin binding env name conflict') END;
END;

CREATE TRIGGER deployment_builtin_bindings_update_guard
BEFORE UPDATE ON deployment_builtin_bindings
BEGIN
  SELECT RAISE(ABORT, 'immutable deployment builtin binding');
END;

CREATE TRIGGER deployment_builtin_bindings_delete_guard
BEFORE DELETE ON deployment_builtin_bindings
WHEN (SELECT state FROM worker_deployments WHERE id = OLD.deployment_id)
  NOT IN ('staging', 'rejected', 'deleting')
BEGIN
  SELECT RAISE(ABORT, 'immutable deployment builtin binding');
END;
