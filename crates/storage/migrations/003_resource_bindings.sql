CREATE TABLE resources (
  id TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  account_id TEXT NOT NULL REFERENCES accounts(id),
  kind TEXT NOT NULL CHECK(kind IN (
    'kv_namespace', 'r2_bucket', 'd1_database', 'do_namespace'
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

CREATE TABLE deployment_bindings (
  id TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  deployment_id TEXT NOT NULL REFERENCES worker_deployments(id),
  name TEXT NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN (
    'kv_namespace', 'r2_bucket', 'd1_database', 'do_namespace'
  )),
  resource_id TEXT NOT NULL REFERENCES resources(id),
  resource_spec_generation INTEGER NOT NULL CHECK(resource_spec_generation >= 1),
  capability_version INTEGER NOT NULL CHECK(capability_version >= 1),
  permissions_json BLOB NOT NULL,
  config_json BLOB NOT NULL,
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms INTEGER NOT NULL,
  UNIQUE(deployment_id, name),
  CHECK(length(name) BETWEEN 1 AND 64)
) STRICT;

CREATE INDEX deployment_bindings_resource
ON deployment_bindings(resource_id, deployment_id, id);

CREATE TABLE resource_referrers (
  resource_id TEXT NOT NULL REFERENCES resources(id),
  referrer_kind TEXT NOT NULL CHECK(referrer_kind IN (
    'deployment_binding', 'queue_dlq', 'queue_consumer',
    'workflow_definition', 'do_class'
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

CREATE TRIGGER deployment_bindings_insert_guard
BEFORE INSERT ON deployment_bindings
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1
    FROM worker_deployments d
    JOIN workers w ON w.id = d.worker_id
    JOIN resources r ON r.id = NEW.resource_id
    WHERE d.id = NEW.deployment_id
      AND d.state = 'staging'
      AND w.account_id = r.account_id
      AND r.state = 'ready'
      AND r.kind = NEW.kind
      AND r.spec_generation = NEW.resource_spec_generation
  ) THEN RAISE(ABORT, 'binding authority invariant') END;
  SELECT CASE WHEN NEW.name GLOB '*[^A-Za-z0-9_]*'
    OR NEW.name GLOB '[^A-Za-z_]*'
    OR NEW.name GLOB 'OPEN_COMPUTE_*'
    OR NEW.name GLOB '__*'
  THEN RAISE(ABORT, 'binding name invariant') END;
  SELECT CASE WHEN EXISTS (
    SELECT 1 FROM deployment_vars
    WHERE deployment_id = NEW.deployment_id AND name = NEW.name
  ) OR EXISTS (
    SELECT 1 FROM deployment_secrets
    WHERE deployment_id = NEW.deployment_id AND name = NEW.name
  ) THEN RAISE(ABORT, 'binding env name conflict') END;
END;

CREATE TRIGGER deployment_bindings_update_guard
BEFORE UPDATE ON deployment_bindings
BEGIN
  SELECT RAISE(ABORT, 'immutable deployment binding');
END;

CREATE TRIGGER deployment_bindings_delete_guard
BEFORE DELETE ON deployment_bindings
WHEN (SELECT state FROM worker_deployments WHERE id = OLD.deployment_id)
  NOT IN ('staging', 'deleting')
BEGIN
  SELECT RAISE(ABORT, 'immutable deployment binding');
END;

CREATE TRIGGER deployment_bindings_referrer_insert
AFTER INSERT ON deployment_bindings
BEGIN
  INSERT INTO resource_referrers
    (resource_id, referrer_kind, referrer_id, created_at_ms)
  VALUES (NEW.resource_id, 'deployment_binding', NEW.id, NEW.created_at_ms);
END;

CREATE TRIGGER deployment_bindings_referrer_delete
AFTER DELETE ON deployment_bindings
BEGIN
  DELETE FROM resource_referrers
  WHERE resource_id = OLD.resource_id
    AND referrer_kind = 'deployment_binding'
    AND referrer_id = OLD.id;
END;

CREATE TRIGGER deployment_binding_referrer_insert_guard
BEFORE INSERT ON resource_referrers
WHEN NEW.referrer_kind = 'deployment_binding'
  AND NOT EXISTS (
    SELECT 1 FROM deployment_bindings
    WHERE id = NEW.referrer_id AND resource_id = NEW.resource_id
  )
BEGIN
  SELECT RAISE(ABORT, 'orphan deployment binding referrer');
END;

CREATE TRIGGER deployment_binding_referrer_delete_guard
BEFORE DELETE ON resource_referrers
WHEN OLD.referrer_kind = 'deployment_binding'
  AND EXISTS (
    SELECT 1
    FROM deployment_bindings b
    JOIN worker_deployments d ON d.id = b.deployment_id
    JOIN workers w ON w.id = d.worker_id
    WHERE b.id = OLD.referrer_id
      AND b.resource_id = OLD.resource_id
      AND w.deleted_at_ms IS NULL
  )
BEGIN
  SELECT RAISE(ABORT, 'live deployment binding referrer');
END;
