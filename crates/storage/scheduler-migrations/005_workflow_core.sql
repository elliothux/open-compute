CREATE TABLE workflow_instances (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL,
  definition_id TEXT NOT NULL,
  definition_name TEXT NOT NULL,
  external_instance_id TEXT NOT NULL,
  version_id TEXT NOT NULL,
  worker_id TEXT NOT NULL,
  deployment_id TEXT NOT NULL,
  worker_code_sha256 BLOB NOT NULL CHECK(length(worker_code_sha256) = 32),
  loader_schema_version INTEGER NOT NULL CHECK(loader_schema_version > 0),
  capability_version INTEGER NOT NULL CHECK(capability_version = 1),
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  class_name TEXT NOT NULL,
  creation_nonce BLOB NOT NULL CHECK(length(creation_nonce) = 32),
  instance_generation INTEGER NOT NULL CHECK(instance_generation >= 1),
  state TEXT NOT NULL CHECK(state IN ('queued','running','complete','errored')),
  input_json BLOB NOT NULL CHECK(length(input_json) <= 1048576),
  output_json BLOB CHECK(output_json IS NULL OR length(output_json) <= 1048576),
  error_json BLOB CHECK(error_json IS NULL OR length(error_json) <= 8192),
  error_code TEXT,
  next_run_at_ms INTEGER,
  run_token BLOB,
  run_claimed_at_ms INTEGER,
  run_lease_until_ms INTEGER,
  completed_step_count INTEGER NOT NULL DEFAULT 0 CHECK(completed_step_count BETWEEN 0 AND 1024),
  state_bytes INTEGER NOT NULL CHECK(state_bytes >= 0),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  terminal_at_ms INTEGER,
  UNIQUE(definition_id,external_instance_id),
  CHECK(
    (state = 'queued' AND next_run_at_ms IS NOT NULL AND run_token IS NULL
      AND run_claimed_at_ms IS NULL AND run_lease_until_ms IS NULL AND terminal_at_ms IS NULL) OR
    (state = 'running' AND next_run_at_ms IS NULL AND run_token IS NOT NULL AND length(run_token) = 32
      AND run_claimed_at_ms IS NOT NULL AND run_lease_until_ms IS NOT NULL
      AND run_lease_until_ms > run_claimed_at_ms AND terminal_at_ms IS NULL) OR
    (state IN ('complete','errored') AND next_run_at_ms IS NULL AND run_token IS NULL
      AND run_claimed_at_ms IS NULL AND run_lease_until_ms IS NULL AND terminal_at_ms IS NOT NULL)
  ),
  CHECK((state = 'complete') = (output_json IS NOT NULL)),
  CHECK((state = 'errored') = (error_json IS NOT NULL)),
  CHECK((state = 'errored') = (error_code IS NOT NULL))
) STRICT;
CREATE INDEX workflow_instances_due ON workflow_instances(next_run_at_ms,created_at_ms,id) WHERE state = 'queued';
CREATE INDEX workflow_instances_expired ON workflow_instances(run_lease_until_ms,id) WHERE state = 'running';
CREATE INDEX workflow_instances_account ON workflow_instances(account_id,definition_id,state);

CREATE TABLE workflow_steps (
  instance_id TEXT NOT NULL REFERENCES workflow_instances(id),
  instance_generation INTEGER NOT NULL CHECK(instance_generation >= 1),
  ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 1023),
  name TEXT NOT NULL CHECK(length(CAST(name AS BLOB)) BETWEEN 1 AND 256),
  name_count INTEGER NOT NULL CHECK(name_count > 0),
  kind TEXT NOT NULL CHECK(kind = 'do'),
  config_json BLOB NOT NULL CHECK(config_json = X'6E756C6C'),
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  state TEXT NOT NULL CHECK(state IN ('pending','running','complete','failed')),
  attempt INTEGER NOT NULL CHECK(attempt = 1),
  run_token BLOB,
  step_token BLOB,
  output_json BLOB CHECK(output_json IS NULL OR length(output_json) <= 1048576),
  error_json BLOB CHECK(error_json IS NULL OR length(error_json) <= 8192),
  error_code TEXT,
  started_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  completed_at_ms INTEGER,
  PRIMARY KEY(instance_id,instance_generation,ordinal),
  UNIQUE(instance_id,instance_generation,name,name_count),
  CHECK((state = 'failed') = (error_code IS NOT NULL)),
  CHECK(
    (state = 'pending' AND run_token IS NULL AND step_token IS NULL AND output_json IS NULL
      AND error_json IS NULL AND completed_at_ms IS NULL) OR
    (state = 'running' AND run_token IS NOT NULL AND step_token IS NOT NULL
      AND length(run_token) = 32 AND length(step_token) = 32
      AND output_json IS NULL AND error_json IS NULL AND completed_at_ms IS NULL) OR
    (state = 'complete' AND run_token IS NULL AND step_token IS NULL AND output_json IS NOT NULL
      AND error_json IS NULL AND completed_at_ms IS NOT NULL) OR
    (state = 'failed' AND run_token IS NULL AND step_token IS NULL AND output_json IS NULL
      AND error_json IS NOT NULL AND completed_at_ms IS NOT NULL)
  )
) WITHOUT ROWID, STRICT;

CREATE TRIGGER workflow_instance_insert_guard BEFORE INSERT ON workflow_instances
WHEN NEW.state != 'queued' OR NEW.instance_generation != 1 OR NEW.completed_step_count != 0
  OR NEW.state_bytes != length(NEW.input_json)
BEGIN SELECT RAISE(ABORT,'workflow initial state'); END;
CREATE TRIGGER workflow_instance_identity_guard BEFORE UPDATE OF id,account_id,definition_id,definition_name,
  external_instance_id,version_id,worker_id,deployment_id,worker_code_sha256,class_name,creation_nonce,
  loader_schema_version,capability_version,descriptor_sha256,
  instance_generation,input_json,created_at_ms ON workflow_instances
BEGIN SELECT RAISE(ABORT,'workflow frozen instance is immutable'); END;
CREATE TRIGGER workflow_instance_terminal_guard BEFORE UPDATE ON workflow_instances
WHEN OLD.state IN ('complete','errored') BEGIN SELECT RAISE(ABORT,'workflow terminal state is immutable'); END;
CREATE TRIGGER workflow_instance_transition_guard BEFORE UPDATE OF state ON workflow_instances
WHEN NEW.state != OLD.state AND NOT (
  (OLD.state = 'queued' AND NEW.state = 'running') OR
  (OLD.state = 'running' AND NEW.state IN ('queued','complete','errored'))
) BEGIN SELECT RAISE(ABORT,'workflow state transition'); END;
CREATE TRIGGER workflow_instance_run_guard BEFORE UPDATE ON workflow_instances
WHEN (OLD.state = 'running' AND NEW.state = 'running' AND
      (NEW.run_token != OLD.run_token OR NEW.run_claimed_at_ms != OLD.run_claimed_at_ms OR
       NEW.run_lease_until_ms < OLD.run_lease_until_ms)) OR
  (OLD.state = 'running' AND NEW.state = 'queued' AND NEW.updated_at_ms < OLD.run_lease_until_ms) OR
  (OLD.state = 'running' AND NEW.state IN ('complete','errored') AND NEW.updated_at_ms >= OLD.run_lease_until_ms)
BEGIN SELECT RAISE(ABORT,'workflow run lease fence'); END;
CREATE TRIGGER workflow_instance_frontier_guard BEFORE UPDATE OF state ON workflow_instances
WHEN (NEW.state = 'queued' AND EXISTS(SELECT 1 FROM workflow_steps WHERE instance_id = OLD.id AND state = 'running'))
  OR (NEW.state = 'complete' AND (NEW.completed_step_count = 0 OR
    EXISTS(SELECT 1 FROM workflow_steps WHERE instance_id = OLD.id AND state != 'complete')))
BEGIN SELECT RAISE(ABORT,'workflow unfinished step frontier'); END;
CREATE TRIGGER workflow_instance_count_guard BEFORE UPDATE OF completed_step_count,state_bytes ON workflow_instances
WHEN NEW.completed_step_count != (SELECT COUNT(*) FROM workflow_steps WHERE instance_id = NEW.id AND state = 'complete')
  OR NEW.state_bytes != length(NEW.input_json) + coalesce(length(NEW.output_json),0) + coalesce(length(NEW.error_json),0)
    + coalesce((SELECT SUM(length(CAST(name AS BLOB)) + length(config_json) + 50
      + coalesce(length(output_json),0) + coalesce(length(error_json),0)) FROM workflow_steps WHERE instance_id = NEW.id),0)
BEGIN SELECT RAISE(ABORT,'workflow state accounting'); END;
CREATE TRIGGER workflow_instance_no_delete BEFORE DELETE ON workflow_instances
BEGIN SELECT RAISE(ABORT,'workflow retention is not supported'); END;

CREATE TRIGGER workflow_step_insert_guard BEFORE INSERT ON workflow_steps
BEGIN
  SELECT CASE WHEN NEW.state != 'running' OR NOT EXISTS (
    SELECT 1 FROM workflow_instances i WHERE i.id = NEW.instance_id AND i.instance_generation = NEW.instance_generation
      AND i.state = 'running' AND i.run_token = NEW.run_token AND i.run_lease_until_ms > NEW.updated_at_ms
      AND NEW.ordinal = i.completed_step_count
  ) OR EXISTS(SELECT 1 FROM workflow_steps WHERE instance_id = NEW.instance_id AND state != 'complete')
    OR NEW.name_count != 1 + (SELECT COUNT(*) FROM workflow_steps WHERE instance_id = NEW.instance_id AND name = NEW.name)
    THEN RAISE(ABORT,'workflow step claim fence') END;
END;
CREATE TRIGGER workflow_step_identity_guard BEFORE UPDATE OF instance_id,instance_generation,ordinal,
  name,name_count,kind,config_json,descriptor_sha256,attempt,started_at_ms ON workflow_steps
BEGIN SELECT RAISE(ABORT,'workflow step descriptor is immutable'); END;
CREATE TRIGGER workflow_step_terminal_guard BEFORE UPDATE ON workflow_steps
WHEN OLD.state IN ('complete','failed') BEGIN SELECT RAISE(ABORT,'workflow step result is immutable'); END;
CREATE TRIGGER workflow_step_transition_guard BEFORE UPDATE ON workflow_steps
WHEN NOT (
  (OLD.state = 'pending' AND NEW.state = 'running' AND EXISTS (
    SELECT 1 FROM workflow_instances i WHERE i.id = OLD.instance_id AND i.instance_generation = OLD.instance_generation
      AND i.state = 'running' AND i.run_token = NEW.run_token AND i.run_lease_until_ms > NEW.updated_at_ms)) OR
  (OLD.state = 'running' AND NEW.state IN ('complete','failed') AND EXISTS (
    SELECT 1 FROM workflow_instances i WHERE i.id = OLD.instance_id AND i.instance_generation = OLD.instance_generation
      AND i.state = 'running' AND i.run_token = OLD.run_token AND i.run_lease_until_ms > NEW.updated_at_ms)) OR
  (OLD.state = 'running' AND NEW.state = 'pending' AND EXISTS (
    SELECT 1 FROM workflow_instances i WHERE i.id = OLD.instance_id AND i.instance_generation = OLD.instance_generation
      AND i.state = 'running' AND i.run_token = OLD.run_token AND i.run_lease_until_ms <= NEW.updated_at_ms))
) BEGIN SELECT RAISE(ABORT,'workflow step transition fence'); END;
-- Logical state accounting: name/config/digest + two 64-bit ordinal/count fields
-- and two bytes for kind; transient lease tokens and physical SQLite pages are excluded.
CREATE TRIGGER workflow_step_insert_accounting AFTER INSERT ON workflow_steps
BEGIN
  UPDATE workflow_instances SET state_bytes = state_bytes + length(CAST(NEW.name AS BLOB)) + length(NEW.config_json) + 50
  WHERE id = NEW.instance_id;
END;
CREATE TRIGGER workflow_step_result_accounting AFTER UPDATE ON workflow_steps
WHEN NEW.state IN ('complete','failed')
BEGIN
  UPDATE workflow_instances SET state_bytes = state_bytes + coalesce(length(NEW.output_json),0) + coalesce(length(NEW.error_json),0),
    completed_step_count = completed_step_count + (NEW.state = 'complete')
  WHERE id = NEW.instance_id;
END;
CREATE TRIGGER workflow_step_no_delete BEFORE DELETE ON workflow_steps
BEGIN SELECT RAISE(ABORT,'workflow step retention is not supported'); END;
