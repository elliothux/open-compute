CREATE TABLE deployment_cron_configs (
  deployment_id      TEXT PRIMARY KEY REFERENCES worker_deployments(id),
  mode               TEXT NOT NULL CHECK(mode IN ('inherit', 'replace')),
  capability_version INTEGER NOT NULL CHECK(capability_version = 1),
  descriptor_sha256  BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms      INTEGER NOT NULL
) STRICT;

CREATE TABLE deployment_cron_declarations (
  id                  TEXT PRIMARY KEY
                      CHECK(length(id) = 36 AND id = lower(id)),
  deployment_id       TEXT NOT NULL REFERENCES worker_deployments(id),
  expression          TEXT NOT NULL CHECK(length(expression) BETWEEN 1 AND 256),
  expression_sha256   BLOB NOT NULL CHECK(length(expression_sha256) = 32),
  parser_version      INTEGER NOT NULL CHECK(parser_version >= 1),
  created_at_ms       INTEGER NOT NULL,
  UNIQUE(deployment_id, expression)
) STRICT;

CREATE TABLE cron_activations (
  id                    TEXT PRIMARY KEY
                        CHECK(length(id) = 36 AND id = lower(id)),
  account_id            TEXT NOT NULL REFERENCES accounts(id),
  worker_id             TEXT NOT NULL REFERENCES workers(id),
  deployment_id         TEXT NOT NULL REFERENCES worker_deployments(id),
  expression            TEXT NOT NULL CHECK(length(expression) BETWEEN 1 AND 256),
  expression_sha256     BLOB NOT NULL CHECK(length(expression_sha256) = 32),
  parser_version        INTEGER NOT NULL CHECK(parser_version >= 1),
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

CREATE TRIGGER deployment_cron_configs_insert_guard
BEFORE INSERT ON deployment_cron_configs
WHEN NOT EXISTS (
  SELECT 1 FROM worker_deployments d
  WHERE d.id = NEW.deployment_id AND d.state = 'staging'
)
BEGIN
  SELECT RAISE(ABORT, 'cron config authority invariant');
END;

CREATE TRIGGER deployment_cron_configs_update_guard
BEFORE UPDATE ON deployment_cron_configs
BEGIN
  SELECT RAISE(ABORT, 'cron deployment config is immutable');
END;

CREATE TRIGGER deployment_cron_configs_delete_guard
BEFORE DELETE ON deployment_cron_configs
WHEN NOT EXISTS (
  SELECT 1 FROM worker_deployments d
  WHERE d.id = OLD.deployment_id AND d.state IN ('staging', 'rejected', 'deleting')
)
BEGIN
  SELECT RAISE(ABORT, 'cron deployment config delete invariant');
END;

CREATE TRIGGER deployment_cron_declarations_insert_guard
BEFORE INSERT ON deployment_cron_declarations
WHEN NOT EXISTS (
  SELECT 1 FROM deployment_cron_configs c
  JOIN worker_deployments d ON d.id = c.deployment_id
  WHERE c.deployment_id = NEW.deployment_id AND c.mode = 'replace' AND d.state = 'staging'
)
BEGIN
  SELECT RAISE(ABORT, 'cron declaration authority invariant');
END;

CREATE TRIGGER deployment_cron_declarations_update_guard
BEFORE UPDATE ON deployment_cron_declarations
BEGIN
  SELECT RAISE(ABORT, 'cron declaration is immutable');
END;

CREATE TRIGGER deployment_cron_declarations_delete_guard
BEFORE DELETE ON deployment_cron_declarations
WHEN NOT EXISTS (
  SELECT 1 FROM worker_deployments d
  WHERE d.id = OLD.deployment_id AND d.state IN ('staging', 'rejected', 'deleting')
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
    SELECT 1 FROM worker_deployments d JOIN workers w ON w.id = d.worker_id
    WHERE d.id = NEW.deployment_id AND d.worker_id = NEW.worker_id
      AND d.state = 'ready' AND w.account_id = NEW.account_id
  ) THEN RAISE(ABORT, 'cron activation authority invariant') END;
END;

CREATE TRIGGER cron_activations_identity_guard
BEFORE UPDATE OF id, account_id, worker_id, expression, expression_sha256,
  parser_version, activation_generation, created_at_ms ON cron_activations
BEGIN
  SELECT RAISE(ABORT, 'cron activation identity is immutable');
END;

CREATE TRIGGER cron_activations_target_guard
BEFORE UPDATE OF deployment_id ON cron_activations
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
  INSERT INTO deployment_referrers(deployment_id, kind, ref_id, created_at_ms)
  VALUES (NEW.deployment_id, 'cron_activation', NEW.id, NEW.created_at_ms);
END;

CREATE TRIGGER cron_activations_referrer_tombstone
AFTER UPDATE OF state ON cron_activations
WHEN NEW.state = 'tombstoned'
BEGIN
  DELETE FROM deployment_referrers
  WHERE deployment_id = NEW.deployment_id AND kind = 'cron_activation' AND ref_id = NEW.id;
END;
