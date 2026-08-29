CREATE TABLE deployment_services (
  deployment_id TEXT NOT NULL REFERENCES worker_deployments(id),
  binding_name TEXT NOT NULL,
  target_worker_id TEXT NOT NULL REFERENCES workers(id),
  entrypoint TEXT,
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms INTEGER NOT NULL,
  PRIMARY KEY(deployment_id, binding_name),
  CHECK(length(binding_name) BETWEEN 1 AND 64),
  CHECK(entrypoint IS NULL OR length(entrypoint) BETWEEN 1 AND 128)
) WITHOUT ROWID, STRICT;

CREATE INDEX deployment_services_target
ON deployment_services(target_worker_id, deployment_id);

CREATE TRIGGER deployment_services_insert_guard
BEFORE INSERT ON deployment_services
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1
    FROM worker_deployments d
    JOIN workers caller ON caller.id = d.worker_id
    JOIN workers target ON target.id = NEW.target_worker_id
    WHERE d.id = NEW.deployment_id
      AND d.state = 'staging'
      AND caller.deleted_at_ms IS NULL
      AND target.deleted_at_ms IS NULL
      AND caller.account_id = target.account_id
  ) THEN RAISE(ABORT, 'service binding authority invariant') END;
  SELECT CASE WHEN NEW.binding_name GLOB '*[^A-Za-z0-9_]*'
    OR NEW.binding_name GLOB '[^A-Za-z_]*'
    OR NEW.binding_name GLOB 'OPEN_COMPUTE_*'
    OR NEW.binding_name GLOB '__*'
  THEN RAISE(ABORT, 'service binding name invariant') END;
  SELECT CASE WHEN NEW.entrypoint IS NOT NULL AND (
    NEW.entrypoint GLOB '*[^A-Za-z0-9_$]*'
    OR NEW.entrypoint GLOB '[^A-Za-z_$]*'
  ) THEN RAISE(ABORT, 'service entrypoint invariant') END;
  SELECT CASE WHEN EXISTS (
    SELECT 1 FROM deployment_vars
    WHERE deployment_id = NEW.deployment_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM deployment_secrets
    WHERE deployment_id = NEW.deployment_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM deployment_bindings
    WHERE deployment_id = NEW.deployment_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM queue_producer_bindings
    WHERE deployment_id = NEW.deployment_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM workflow_bindings
    WHERE deployment_id = NEW.deployment_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM deployment_assets
    WHERE deployment_id = NEW.deployment_id AND binding_name = NEW.binding_name
  ) THEN RAISE(ABORT, 'service env name conflict') END;
END;

CREATE TRIGGER deployment_services_update_guard
BEFORE UPDATE ON deployment_services
BEGIN
  SELECT RAISE(ABORT, 'immutable deployment service');
END;

CREATE TRIGGER deployment_services_delete_guard
BEFORE DELETE ON deployment_services
WHEN (SELECT state FROM worker_deployments WHERE id = OLD.deployment_id)
  NOT IN ('staging', 'rejected', 'deleting')
BEGIN
  SELECT RAISE(ABORT, 'immutable deployment service');
END;

CREATE TRIGGER deployment_vars_service_name_guard
BEFORE INSERT ON deployment_vars
WHEN EXISTS (
  SELECT 1 FROM deployment_services
  WHERE deployment_id = NEW.deployment_id AND binding_name = NEW.name
)
BEGIN
  SELECT RAISE(ABORT, 'service env name conflict');
END;

CREATE TRIGGER deployment_secrets_service_name_guard
BEFORE INSERT ON deployment_secrets
WHEN EXISTS (
  SELECT 1 FROM deployment_services
  WHERE deployment_id = NEW.deployment_id AND binding_name = NEW.name
)
BEGIN
  SELECT RAISE(ABORT, 'service env name conflict');
END;

CREATE TRIGGER deployment_bindings_service_name_guard
BEFORE INSERT ON deployment_bindings
WHEN EXISTS (
  SELECT 1 FROM deployment_services
  WHERE deployment_id = NEW.deployment_id AND binding_name = NEW.name
)
BEGIN
  SELECT RAISE(ABORT, 'service env name conflict');
END;

CREATE TRIGGER queue_producer_bindings_service_name_guard
BEFORE INSERT ON queue_producer_bindings
WHEN EXISTS (
  SELECT 1 FROM deployment_services
  WHERE deployment_id = NEW.deployment_id AND binding_name = NEW.name
)
BEGIN
  SELECT RAISE(ABORT, 'service env name conflict');
END;

CREATE TRIGGER workflow_bindings_service_name_guard
BEFORE INSERT ON workflow_bindings
WHEN EXISTS (
  SELECT 1 FROM deployment_services
  WHERE deployment_id = NEW.deployment_id AND binding_name = NEW.name
)
BEGIN
  SELECT RAISE(ABORT, 'service env name conflict');
END;

CREATE TRIGGER deployment_assets_service_name_guard
BEFORE INSERT ON deployment_assets
WHEN NEW.binding_name IS NOT NULL AND EXISTS (
  SELECT 1 FROM deployment_services
  WHERE deployment_id = NEW.deployment_id AND binding_name = NEW.binding_name
)
BEGIN
  SELECT RAISE(ABORT, 'service env name conflict');
END;
