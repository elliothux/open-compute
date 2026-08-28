-- Forward-only Workflow subgraph rebuild. Foreign keys remain ON throughout.
CREATE TEMP TABLE saved_workflow_instances AS SELECT * FROM workflow_instances;
CREATE TEMP TABLE saved_workflow_steps AS SELECT * FROM workflow_steps;
DROP TRIGGER workflow_instance_insert_guard;
DROP TRIGGER workflow_instance_identity_guard;
DROP TRIGGER workflow_instance_terminal_guard;
DROP TRIGGER workflow_instance_transition_guard;
DROP TRIGGER workflow_instance_run_guard;
DROP TRIGGER workflow_instance_frontier_guard;
DROP TRIGGER workflow_instance_count_guard;
DROP TRIGGER workflow_instance_no_delete;
DROP TRIGGER workflow_step_insert_guard;
DROP TRIGGER workflow_step_identity_guard;
DROP TRIGGER workflow_step_terminal_guard;
DROP TRIGGER workflow_step_transition_guard;
DROP TRIGGER workflow_step_insert_accounting;
DROP TRIGGER workflow_step_result_accounting;
DROP TRIGGER workflow_step_no_delete;
DROP TABLE workflow_steps;
DROP TABLE workflow_instances;

CREATE TABLE workflow_instances (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL,
  definition_id TEXT NOT NULL,
  definition_name TEXT NOT NULL,
  external_instance_id TEXT NOT NULL,
  version_id TEXT NOT NULL,
  worker_id TEXT NOT NULL,
  deployment_id TEXT NOT NULL,
  worker_code_sha256 BLOB NOT NULL CHECK(length(worker_code_sha256)=32),
  loader_schema_version INTEGER NOT NULL CHECK(loader_schema_version>0),
  capability_version INTEGER NOT NULL CHECK(capability_version IN (1,2)),
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256)=32),
  class_name TEXT NOT NULL,
  creation_nonce BLOB NOT NULL CHECK(length(creation_nonce)=32),
  instance_generation INTEGER NOT NULL CHECK(instance_generation>=1),
  state TEXT NOT NULL CHECK(state IN ('queued','running','waiting','paused','complete','errored','terminated')),
  input_json BLOB NOT NULL CHECK(length(input_json)<=1048576),
  output_json BLOB CHECK(output_json IS NULL OR length(output_json)<=1048576),
  error_json BLOB CHECK(error_json IS NULL OR length(error_json)<=8192),
  error_code TEXT,
  next_run_at_ms INTEGER,
  run_token BLOB,
  run_claimed_at_ms INTEGER,
  run_lease_until_ms INTEGER,
  completed_step_count INTEGER NOT NULL DEFAULT 0 CHECK(completed_step_count BETWEEN 0 AND 1024),
  state_bytes INTEGER NOT NULL CHECK(state_bytes>=0),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  terminal_at_ms INTEGER,
  pause_requested INTEGER NOT NULL DEFAULT 0 CHECK(pause_requested IN (0,1)),
  yield_requested INTEGER NOT NULL DEFAULT 0 CHECK(yield_requested IN (0,1)),
  next_wake_at_ms INTEGER,
  registered_step_count INTEGER NOT NULL DEFAULT 0 CHECK(registered_step_count BETWEEN 0 AND 1024),
  settled_step_count INTEGER NOT NULL DEFAULT 0 CHECK(settled_step_count BETWEEN 0 AND 1024),
  success_retention_ms INTEGER,
  error_retention_ms INTEGER,
  expires_at_ms INTEGER,
  last_restart_operation_id TEXT,
  event_count INTEGER NOT NULL DEFAULT 0 CHECK(event_count>=0),
  event_bytes INTEGER NOT NULL DEFAULT 0 CHECK(event_bytes>=0),
  next_event_seq INTEGER NOT NULL DEFAULT 1 CHECK(next_event_seq>=1),
  has_activated INTEGER NOT NULL DEFAULT 0 CHECK(has_activated IN (0,1)),
  UNIQUE(definition_id,external_instance_id),
  UNIQUE(id,instance_generation),
  CHECK(
    (state='queued' AND next_run_at_ms IS NOT NULL AND run_token IS NULL
      AND run_claimed_at_ms IS NULL AND run_lease_until_ms IS NULL AND terminal_at_ms IS NULL) OR
    (state='running' AND next_run_at_ms IS NULL AND run_token IS NOT NULL AND length(run_token)=32
      AND run_claimed_at_ms IS NOT NULL AND run_lease_until_ms IS NOT NULL
      AND run_lease_until_ms>run_claimed_at_ms AND terminal_at_ms IS NULL) OR
    (state IN ('waiting','paused') AND next_run_at_ms IS NULL AND run_token IS NULL
      AND run_claimed_at_ms IS NULL AND run_lease_until_ms IS NULL AND terminal_at_ms IS NULL) OR
    (state IN ('complete','errored','terminated') AND next_run_at_ms IS NULL AND run_token IS NULL
      AND run_claimed_at_ms IS NULL AND run_lease_until_ms IS NULL AND terminal_at_ms IS NOT NULL
      AND next_wake_at_ms IS NULL)
  ),
  CHECK((state='complete')=(output_json IS NOT NULL)),
  CHECK((state='errored')=(error_json IS NOT NULL)),
  CHECK((state='errored')=(error_code IS NOT NULL)),
  CHECK(state='running' OR (pause_requested=0 AND yield_requested=0)),
  CHECK(
    (capability_version=1 AND instance_generation=1 AND state IN ('queued','running','complete','errored')
      AND pause_requested=0 AND yield_requested=0 AND next_wake_at_ms IS NULL
      AND registered_step_count=0 AND settled_step_count=0
      AND success_retention_ms IS NULL AND error_retention_ms IS NULL AND expires_at_ms IS NULL
      AND last_restart_operation_id IS NULL AND event_count=0 AND event_bytes=0 AND next_event_seq=1 AND has_activated=0) OR
    (capability_version=2 AND success_retention_ms IS NOT NULL AND error_retention_ms IS NOT NULL
      AND success_retention_ms BETWEEN 3600000 AND 31536000000
      AND error_retention_ms BETWEEN 3600000 AND 31536000000
      AND completed_step_count<=settled_step_count AND settled_step_count<=registered_step_count
      AND ((terminal_at_ms IS NULL AND expires_at_ms IS NULL) OR
        (terminal_at_ms IS NOT NULL AND expires_at_ms IS NOT NULL AND expires_at_ms<=9007199254740991
          AND expires_at_ms=terminal_at_ms+CASE WHEN state='complete' THEN success_retention_ms ELSE error_retention_ms END)))
  )
) STRICT;
CREATE INDEX workflow_instances_due ON workflow_instances(next_run_at_ms,created_at_ms,id) WHERE state='queued';
CREATE INDEX workflow_instances_expired ON workflow_instances(run_lease_until_ms,id) WHERE state='running';
CREATE INDEX workflow_instances_account ON workflow_instances(account_id,definition_id,state);
CREATE INDEX workflow_instances_waiting ON workflow_instances(next_wake_at_ms,id) WHERE state='waiting';
CREATE INDEX workflow_instances_retention ON workflow_instances(expires_at_ms,id) WHERE capability_version=2 AND state IN ('complete','errored','terminated');

CREATE TABLE workflow_steps (
  instance_id TEXT NOT NULL,
  instance_generation INTEGER NOT NULL CHECK(instance_generation>=1),
  ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 1023),
  name TEXT NOT NULL CHECK(length(CAST(name AS BLOB)) BETWEEN 1 AND 256),
  name_count INTEGER NOT NULL CHECK(name_count>0),
  kind TEXT NOT NULL CHECK(kind IN ('do','sleep','sleep_until','wait_event')),
  config_json BLOB NOT NULL CHECK(length(config_json)<=4096),
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256)=32),
  state TEXT NOT NULL CHECK(state IN ('pending','running','retry_wait','waiting','complete','failed','cancelled')),
  attempt INTEGER NOT NULL CHECK(attempt BETWEEN 0 AND 101),
  run_token BLOB,
  step_token BLOB,
  output_json BLOB CHECK(output_json IS NULL OR length(output_json)<=CASE WHEN kind='wait_event' THEN 1049600 ELSE 1048576 END),
  error_json BLOB CHECK(error_json IS NULL OR length(error_json)<=8192),
  error_code TEXT,
  started_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  completed_at_ms INTEGER,
  config_sha256 BLOB CHECK(config_sha256 IS NULL OR length(config_sha256)=32),
  batch_first_ordinal INTEGER NOT NULL DEFAULT 0 CHECK(batch_first_ordinal BETWEEN 0 AND 1023),
  batch_size INTEGER NOT NULL DEFAULT 1 CHECK(batch_size BETWEEN 1 AND 16),
  dependency_count INTEGER NOT NULL DEFAULT 0 CHECK(dependency_count BETWEEN 0 AND 16),
  attempt_started_at_ms INTEGER,
  attempt_deadline_at_ms INTEGER,
  due_at_ms INTEGER,
  cancelled_at_ms INTEGER,
  event_buffer_ceiling INTEGER,
  consumed_event_seq INTEGER,
  PRIMARY KEY(instance_id,instance_generation,ordinal),
  UNIQUE(instance_id,instance_generation,kind,name,name_count),
  FOREIGN KEY(instance_id,instance_generation) REFERENCES workflow_instances(id,instance_generation),
  CHECK((run_token IS NOT NULL)=(state='running')),
  CHECK((step_token IS NOT NULL)=(state='running')),
  CHECK(run_token IS NULL OR (length(run_token)=32 AND length(step_token)=32)),
  CHECK(state!='running' OR kind='do'),
  CHECK((completed_at_ms IS NOT NULL)=(state IN ('complete','failed'))),
  CHECK((cancelled_at_ms IS NOT NULL)=(state='cancelled')),
  CHECK((due_at_ms IS NOT NULL)=(state IN ('retry_wait','waiting'))),
  CHECK(state!='retry_wait' OR (kind='do' AND error_json IS NOT NULL AND error_code IS NOT NULL AND due_at_ms>=updated_at_ms AND due_at_ms-updated_at_ms<=86400000)),
  CHECK(state!='waiting' OR kind!='do'),
  CHECK(state!='failed' OR (error_json IS NOT NULL AND error_code IS NOT NULL)),
  CHECK(state IN ('failed','retry_wait') OR (error_json IS NULL AND error_code IS NULL)),
  CHECK((output_json IS NOT NULL)=(state='complete' AND kind IN ('do','wait_event'))),
  CHECK(kind='do' OR attempt=0),
  CHECK((attempt_started_at_ms IS NULL)=(attempt_deadline_at_ms IS NULL)),
  CHECK(attempt_deadline_at_ms IS NULL OR attempt_deadline_at_ms>attempt_started_at_ms),
  CHECK(event_buffer_ceiling IS NULL OR (kind='wait_event' AND event_buffer_ceiling>=0)),
  CHECK(consumed_event_seq IS NULL OR (kind='wait_event' AND state='complete' AND consumed_event_seq>=1)),
  CHECK(
    (config_sha256 IS NULL AND kind='do' AND config_json=X'6E756C6C' AND attempt=1
      AND batch_first_ordinal=0 AND batch_size=1 AND dependency_count=0
      AND attempt_started_at_ms IS NULL AND attempt_deadline_at_ms IS NULL AND due_at_ms IS NULL
      AND cancelled_at_ms IS NULL AND event_buffer_ceiling IS NULL AND consumed_event_seq IS NULL
      AND state IN ('pending','running','complete','failed')) OR
    (config_sha256 IS NOT NULL AND json_valid(CAST(config_json AS TEXT))
      AND json_type(CAST(config_json AS TEXT))='object'
      AND ordinal>=batch_first_ordinal AND ordinal<batch_first_ordinal+batch_size AND batch_first_ordinal+batch_size<=1024
      AND (kind='do' OR batch_size=1)
      AND (kind!='do' OR ((attempt=0 AND state IN ('pending','cancelled') AND attempt_started_at_ms IS NULL)
        OR (attempt>=1 AND attempt_started_at_ms IS NOT NULL)))
      AND (kind='do' OR (attempt_started_at_ms IS NULL AND state IN ('waiting','complete','failed','cancelled')))
      AND (kind='wait_event' OR event_buffer_ceiling IS NULL)
      AND (kind!='wait_event' OR (event_buffer_ceiling IS NOT NULL AND (state!='complete' OR consumed_event_seq IS NOT NULL))))
  )
) WITHOUT ROWID, STRICT;

CREATE TABLE workflow_step_dependencies (
  instance_id TEXT NOT NULL,
  instance_generation INTEGER NOT NULL,
  child_ordinal INTEGER NOT NULL,
  parent_ordinal INTEGER NOT NULL CHECK(parent_ordinal<child_ordinal),
  PRIMARY KEY(instance_id,instance_generation,child_ordinal,parent_ordinal),
  FOREIGN KEY(instance_id,instance_generation,child_ordinal) REFERENCES workflow_steps(instance_id,instance_generation,ordinal),
  FOREIGN KEY(instance_id,instance_generation,parent_ordinal) REFERENCES workflow_steps(instance_id,instance_generation,ordinal)
) WITHOUT ROWID, STRICT;
CREATE TABLE workflow_events (
  instance_id TEXT NOT NULL,
  instance_generation INTEGER NOT NULL,
  event_seq INTEGER NOT NULL CHECK(event_seq>=1),
  type TEXT NOT NULL CHECK(length(CAST(type AS BLOB)) BETWEEN 1 AND 100),
  payload_json BLOB NOT NULL CHECK(length(payload_json)<=1048576),
  accepted_at_ms INTEGER NOT NULL,
  logical_bytes INTEGER NOT NULL CHECK(logical_bytes=length(CAST(type AS BLOB))+length(payload_json)+32),
  PRIMARY KEY(instance_id,instance_generation,event_seq),
  FOREIGN KEY(instance_id,instance_generation) REFERENCES workflow_instances(id,instance_generation)
) WITHOUT ROWID, STRICT;
CREATE INDEX workflow_events_fifo ON workflow_events(instance_id,instance_generation,type,event_seq);

-- This context exists only inside one owner transaction. It grants deletion for one
-- exact operation/UUID/nonce/generation, never a database-wide allow-delete switch.
CREATE TABLE workflow_mutation_context (
  instance_id TEXT PRIMARY KEY,
  operation_id TEXT NOT NULL UNIQUE,
  creation_nonce BLOB NOT NULL CHECK(length(creation_nonce)=32),
  expected_generation INTEGER NOT NULL CHECK(expected_generation>=1),
  target_generation INTEGER NOT NULL CHECK(target_generation>=1),
  kind TEXT NOT NULL CHECK(kind IN ('restart','purge','acknowledge_purge')),
  authorized_at_ms INTEGER NOT NULL,
  CHECK((kind='restart' AND expected_generation<9223372036854775807 AND target_generation=expected_generation+1)
    OR (kind IN ('purge','acknowledge_purge') AND target_generation=expected_generation))
) STRICT;
CREATE TABLE workflow_gc_receipts (
  operation_id TEXT PRIMARY KEY,
  instance_id TEXT NOT NULL UNIQUE,
  creation_nonce BLOB NOT NULL CHECK(length(creation_nonce)=32),
  instance_generation INTEGER NOT NULL CHECK(instance_generation>=1),
  deleted_at_ms INTEGER NOT NULL
) STRICT;

INSERT INTO workflow_instances(id,account_id,definition_id,definition_name,external_instance_id,version_id,
  worker_id,deployment_id,worker_code_sha256,loader_schema_version,capability_version,descriptor_sha256,class_name,
  creation_nonce,instance_generation,state,input_json,output_json,error_json,error_code,next_run_at_ms,run_token,
  run_claimed_at_ms,run_lease_until_ms,completed_step_count,state_bytes,created_at_ms,updated_at_ms,terminal_at_ms)
  SELECT * FROM saved_workflow_instances;
INSERT INTO workflow_steps(instance_id,instance_generation,ordinal,name,name_count,kind,config_json,descriptor_sha256,
  state,attempt,run_token,step_token,output_json,error_json,error_code,started_at_ms,updated_at_ms,completed_at_ms)
  SELECT * FROM saved_workflow_steps;
CREATE TRIGGER workflow_v1_instance_insert_guard BEFORE INSERT ON workflow_instances
WHEN NEW.capability_version=1 AND (NEW.state != 'queued' OR NEW.instance_generation != 1 OR NEW.completed_step_count != 0
  OR NEW.state_bytes != length(NEW.input_json))
BEGIN SELECT RAISE(ABORT,'workflow initial state'); END;

CREATE TRIGGER workflow_v1_instance_identity_guard BEFORE UPDATE OF id,account_id,definition_id,definition_name,
  external_instance_id,version_id,worker_id,deployment_id,worker_code_sha256,class_name,creation_nonce,
  loader_schema_version,capability_version,descriptor_sha256,
  instance_generation,input_json,created_at_ms ON workflow_instances
WHEN OLD.capability_version=1
BEGIN SELECT RAISE(ABORT,'workflow frozen instance is immutable'); END;

CREATE TRIGGER workflow_v1_instance_terminal_guard BEFORE UPDATE ON workflow_instances
WHEN NEW.capability_version=1 AND (OLD.state IN ('complete','errored'))
BEGIN SELECT RAISE(ABORT,'workflow terminal state is immutable'); END;

CREATE TRIGGER workflow_v1_instance_transition_guard BEFORE UPDATE OF state ON workflow_instances
WHEN NEW.capability_version=1 AND (NEW.state != OLD.state AND NOT (
  (OLD.state = 'queued' AND NEW.state = 'running') OR
  (OLD.state = 'running' AND NEW.state IN ('queued','complete','errored'))
))
BEGIN SELECT RAISE(ABORT,'workflow state transition'); END;

CREATE TRIGGER workflow_v1_instance_run_guard BEFORE UPDATE ON workflow_instances
WHEN NEW.capability_version=1 AND ((OLD.state = 'running' AND NEW.state = 'running' AND
      (NEW.run_token != OLD.run_token OR NEW.run_claimed_at_ms != OLD.run_claimed_at_ms OR
       NEW.run_lease_until_ms < OLD.run_lease_until_ms)) OR
  (OLD.state = 'running' AND NEW.state = 'queued' AND NEW.updated_at_ms < OLD.run_lease_until_ms) OR
  (OLD.state = 'running' AND NEW.state IN ('complete','errored') AND NEW.updated_at_ms >= OLD.run_lease_until_ms))
BEGIN SELECT RAISE(ABORT,'workflow run lease fence'); END;

CREATE TRIGGER workflow_v1_instance_frontier_guard BEFORE UPDATE OF state ON workflow_instances
WHEN NEW.capability_version=1 AND ((NEW.state = 'queued' AND EXISTS(SELECT 1 FROM workflow_steps WHERE instance_id = OLD.id AND state = 'running'))
  OR (NEW.state = 'complete' AND (NEW.completed_step_count = 0 OR
    EXISTS(SELECT 1 FROM workflow_steps WHERE instance_id = OLD.id AND state != 'complete'))))
BEGIN SELECT RAISE(ABORT,'workflow unfinished step frontier'); END;

CREATE TRIGGER workflow_v1_instance_count_guard BEFORE UPDATE OF completed_step_count,state_bytes ON workflow_instances
WHEN NEW.capability_version=1 AND (NEW.completed_step_count != (SELECT COUNT(*) FROM workflow_steps WHERE instance_id = NEW.id AND state = 'complete')
  OR NEW.state_bytes != length(NEW.input_json) + coalesce(length(NEW.output_json),0) + coalesce(length(NEW.error_json),0)
    + coalesce((SELECT SUM(length(CAST(name AS BLOB)) + length(config_json) + 50
      + coalesce(length(output_json),0) + coalesce(length(error_json),0)) FROM workflow_steps WHERE instance_id = NEW.id),0))
BEGIN SELECT RAISE(ABORT,'workflow state accounting'); END;

CREATE TRIGGER workflow_v1_instance_no_delete BEFORE DELETE ON workflow_instances
WHEN OLD.capability_version=1
BEGIN SELECT RAISE(ABORT,'workflow retention is not supported'); END;

CREATE TRIGGER workflow_v1_step_insert_guard BEFORE INSERT ON workflow_steps
WHEN (SELECT capability_version FROM workflow_instances WHERE id=NEW.instance_id)=1
BEGIN
  SELECT CASE WHEN NEW.state != 'running' OR NOT EXISTS (
    SELECT 1 FROM workflow_instances i WHERE i.id = NEW.instance_id AND i.instance_generation = NEW.instance_generation
      AND i.state = 'running' AND i.run_token = NEW.run_token AND i.run_lease_until_ms > NEW.updated_at_ms
      AND NEW.ordinal = i.completed_step_count
  ) OR EXISTS(SELECT 1 FROM workflow_steps WHERE instance_id = NEW.instance_id AND state != 'complete')
    OR NEW.name_count != 1 + (SELECT COUNT(*) FROM workflow_steps WHERE instance_id = NEW.instance_id AND name = NEW.name)
    THEN RAISE(ABORT,'workflow step claim fence') END;
END;

CREATE TRIGGER workflow_v1_step_identity_guard BEFORE UPDATE OF instance_id,instance_generation,ordinal,
  name,name_count,kind,config_json,descriptor_sha256,attempt,started_at_ms ON workflow_steps
WHEN (SELECT capability_version FROM workflow_instances WHERE id=OLD.instance_id)=1
BEGIN SELECT RAISE(ABORT,'workflow step descriptor is immutable'); END;

CREATE TRIGGER workflow_v1_step_terminal_guard BEFORE UPDATE ON workflow_steps
WHEN (SELECT capability_version FROM workflow_instances WHERE id=NEW.instance_id)=1 AND (OLD.state IN ('complete','failed'))
BEGIN SELECT RAISE(ABORT,'workflow step result is immutable'); END;

CREATE TRIGGER workflow_v1_step_transition_guard BEFORE UPDATE ON workflow_steps
WHEN (SELECT capability_version FROM workflow_instances WHERE id=NEW.instance_id)=1 AND (NOT (
  (OLD.state = 'pending' AND NEW.state = 'running' AND EXISTS (
    SELECT 1 FROM workflow_instances i WHERE i.id = OLD.instance_id AND i.instance_generation = OLD.instance_generation
      AND i.state = 'running' AND i.run_token = NEW.run_token AND i.run_lease_until_ms > NEW.updated_at_ms)) OR
  (OLD.state = 'running' AND NEW.state IN ('complete','failed') AND EXISTS (
    SELECT 1 FROM workflow_instances i WHERE i.id = OLD.instance_id AND i.instance_generation = OLD.instance_generation
      AND i.state = 'running' AND i.run_token = OLD.run_token AND i.run_lease_until_ms > NEW.updated_at_ms)) OR
  (OLD.state = 'running' AND NEW.state = 'pending' AND EXISTS (
    SELECT 1 FROM workflow_instances i WHERE i.id = OLD.instance_id AND i.instance_generation = OLD.instance_generation
      AND i.state = 'running' AND i.run_token = OLD.run_token AND i.run_lease_until_ms <= NEW.updated_at_ms))
))
BEGIN SELECT RAISE(ABORT,'workflow step transition fence'); END;

-- Logical state accounting: name/config/digest + two 64-bit ordinal/count fields
-- and two bytes for kind; transient lease tokens and physical SQLite pages are excluded.
CREATE TRIGGER workflow_v1_step_insert_accounting AFTER INSERT ON workflow_steps
WHEN (SELECT capability_version FROM workflow_instances WHERE id=NEW.instance_id)=1
BEGIN
  UPDATE workflow_instances SET state_bytes = state_bytes + length(CAST(NEW.name AS BLOB)) + length(NEW.config_json) + 50
  WHERE id = NEW.instance_id;
END;

CREATE TRIGGER workflow_v1_step_result_accounting AFTER UPDATE ON workflow_steps
WHEN (SELECT capability_version FROM workflow_instances WHERE id=NEW.instance_id)=1 AND (NEW.state IN ('complete','failed'))
BEGIN
  UPDATE workflow_instances SET state_bytes = state_bytes + coalesce(length(NEW.output_json),0) + coalesce(length(NEW.error_json),0),
    completed_step_count = completed_step_count + (NEW.state = 'complete')
  WHERE id = NEW.instance_id;
END;

CREATE TRIGGER workflow_v1_step_no_delete BEFORE DELETE ON workflow_steps
WHEN (SELECT capability_version FROM workflow_instances WHERE id=OLD.instance_id)=1
BEGIN SELECT RAISE(ABORT,'workflow step retention is not supported'); END;

-- Logical accounting contract: share/workflow-accounting-v2.json. Transient tokens
-- and physical SQLite pages are excluded. Event envelopes count their full bytes.
CREATE VIEW workflow_v2_accounting AS SELECT i.id,
  (SELECT COUNT(*) FROM workflow_steps s WHERE s.instance_id=i.id) AS registered,
  (SELECT COUNT(*) FROM workflow_steps s WHERE s.instance_id=i.id AND s.state IN ('complete','failed')) AS settled,
  (SELECT COUNT(*) FROM workflow_steps s WHERE s.instance_id=i.id AND s.state='complete') AS completed,
  (SELECT COUNT(*) FROM workflow_events e WHERE e.instance_id=i.id) AS event_count,
  coalesce((SELECT SUM(logical_bytes) FROM workflow_events e WHERE e.instance_id=i.id),0) AS event_bytes,
  coalesce((SELECT SUM(160+length(CAST(s.name AS BLOB))+length(s.config_json)
    +coalesce(length(s.output_json),0)+coalesce(length(s.error_json),0)) FROM workflow_steps s WHERE s.instance_id=i.id),0)
    +16*(SELECT COUNT(*) FROM workflow_step_dependencies d WHERE d.instance_id=i.id)
    +coalesce((SELECT SUM(logical_bytes) FROM workflow_events e WHERE e.instance_id=i.id),0) AS history_bytes,
  (SELECT MIN(CASE WHEN s.state IN ('pending','running') THEN s.attempt_deadline_at_ms
    WHEN s.state IN ('waiting','retry_wait') THEN s.due_at_ms END) FROM workflow_steps s WHERE s.instance_id=i.id) AS next_wake
  FROM workflow_instances i WHERE i.capability_version=2;

CREATE TRIGGER workflow_context_insert_guard BEFORE INSERT ON workflow_mutation_context
WHEN NOT (
  (NEW.kind IN ('restart','purge') AND EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=NEW.instance_id
    AND i.capability_version=2 AND i.creation_nonce=NEW.creation_nonce AND i.instance_generation=NEW.expected_generation
    AND ((NEW.kind='restart' AND (i.expires_at_ms IS NULL OR i.expires_at_ms>NEW.authorized_at_ms))
      OR (NEW.kind='purge' AND i.state IN ('complete','errored','terminated') AND i.run_token IS NULL
        AND i.expires_at_ms<=NEW.authorized_at_ms)))) OR
  (NEW.kind='acknowledge_purge' AND EXISTS(SELECT 1 FROM workflow_gc_receipts r WHERE r.operation_id=NEW.operation_id
    AND r.instance_id=NEW.instance_id AND r.creation_nonce=NEW.creation_nonce AND r.instance_generation=NEW.expected_generation))
) BEGIN SELECT RAISE(ABORT,'workflow operation context identity'); END;
CREATE TRIGGER workflow_context_immutable BEFORE UPDATE ON workflow_mutation_context
BEGIN SELECT RAISE(ABORT,'workflow operation context is immutable'); END;
CREATE TRIGGER workflow_context_delete_guard BEFORE DELETE ON workflow_mutation_context
WHEN NOT (
  (OLD.kind='restart' AND EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=OLD.instance_id
    AND i.creation_nonce=OLD.creation_nonce AND i.instance_generation=OLD.target_generation AND i.last_restart_operation_id=OLD.operation_id)) OR
  (OLD.kind='purge' AND EXISTS(SELECT 1 FROM workflow_gc_receipts r WHERE r.operation_id=OLD.operation_id
    AND r.instance_id=OLD.instance_id AND r.creation_nonce=OLD.creation_nonce AND r.instance_generation=OLD.expected_generation)) OR
  (OLD.kind='acknowledge_purge' AND NOT EXISTS(SELECT 1 FROM workflow_gc_receipts WHERE operation_id=OLD.operation_id))
) BEGIN SELECT RAISE(ABORT,'workflow operation did not commit'); END;
CREATE TRIGGER workflow_receipt_insert_guard BEFORE INSERT ON workflow_gc_receipts
WHEN EXISTS(SELECT 1 FROM workflow_instances WHERE id=NEW.instance_id) OR NOT EXISTS(
  SELECT 1 FROM workflow_mutation_context c WHERE c.kind='purge' AND c.operation_id=NEW.operation_id
    AND c.instance_id=NEW.instance_id AND c.creation_nonce=NEW.creation_nonce AND c.expected_generation=NEW.instance_generation
    AND c.authorized_at_ms=NEW.deleted_at_ms)
BEGIN SELECT RAISE(ABORT,'workflow purge receipt requires exact deletion'); END;
CREATE TRIGGER workflow_receipt_immutable BEFORE UPDATE ON workflow_gc_receipts
BEGIN SELECT RAISE(ABORT,'workflow purge receipt is immutable'); END;
CREATE TRIGGER workflow_receipt_delete_guard BEFORE DELETE ON workflow_gc_receipts
WHEN NOT EXISTS(SELECT 1 FROM workflow_mutation_context c WHERE c.kind='acknowledge_purge'
  AND c.operation_id=OLD.operation_id AND c.instance_id=OLD.instance_id AND c.creation_nonce=OLD.creation_nonce
  AND c.expected_generation=OLD.instance_generation)
BEGIN SELECT RAISE(ABORT,'workflow purge is not acknowledged'); END;

CREATE TRIGGER workflow_v2_instance_insert_guard BEFORE INSERT ON workflow_instances
WHEN NEW.capability_version=2 AND (NEW.state!='queued' OR NEW.instance_generation!=1 OR NEW.completed_step_count!=0
  OR NEW.registered_step_count!=0 OR NEW.settled_step_count!=0 OR NEW.event_count!=0 OR NEW.event_bytes!=0
  OR NEW.next_event_seq!=1 OR NEW.has_activated!=0 OR NEW.last_restart_operation_id IS NOT NULL
  OR NEW.next_wake_at_ms IS NOT NULL OR NEW.state_bytes!=256+length(NEW.input_json)
    +length(CAST(NEW.definition_name AS BLOB))+length(CAST(NEW.external_instance_id AS BLOB))+length(CAST(NEW.class_name AS BLOB)))
BEGIN SELECT RAISE(ABORT,'workflow durable initial state'); END;
CREATE TRIGGER workflow_v2_instance_identity_guard BEFORE UPDATE OF id,account_id,definition_id,definition_name,
  external_instance_id,version_id,worker_id,deployment_id,worker_code_sha256,class_name,creation_nonce,
  loader_schema_version,capability_version,descriptor_sha256,input_json,created_at_ms,success_retention_ms,error_retention_ms
ON workflow_instances WHEN OLD.capability_version=2
BEGIN SELECT RAISE(ABORT,'workflow durable identity is immutable'); END;
CREATE TRIGGER workflow_v2_generation_guard BEFORE UPDATE OF instance_generation,last_restart_operation_id ON workflow_instances
WHEN OLD.capability_version=2 AND NOT EXISTS(SELECT 1 FROM workflow_mutation_context c WHERE c.instance_id=OLD.id
  AND c.kind='restart' AND c.creation_nonce=OLD.creation_nonce AND c.expected_generation=OLD.instance_generation
  AND c.target_generation=NEW.instance_generation AND NEW.last_restart_operation_id=c.operation_id
  AND NEW.state='queued' AND NEW.registered_step_count=0 AND NEW.event_count=0 AND NEW.next_event_seq=1 AND NEW.has_activated=0)
BEGIN SELECT RAISE(ABORT,'workflow generation requires exact restart'); END;
CREATE TRIGGER workflow_v2_instance_terminal_guard BEFORE UPDATE ON workflow_instances
WHEN OLD.capability_version=2 AND OLD.state IN ('complete','errored','terminated') AND NOT EXISTS(
  SELECT 1 FROM workflow_mutation_context c WHERE c.instance_id=OLD.id AND c.creation_nonce=OLD.creation_nonce
    AND c.expected_generation=OLD.instance_generation AND c.kind IN ('restart','purge'))
BEGIN SELECT RAISE(ABORT,'workflow terminal history is immutable'); END;
CREATE TRIGGER workflow_v2_instance_transition_guard BEFORE UPDATE OF state ON workflow_instances
WHEN OLD.capability_version=2 AND NEW.state!=OLD.state AND NOT (
  (OLD.state='queued' AND NEW.state IN ('running','paused','terminated')) OR
  (OLD.state='running' AND NEW.state IN ('queued','waiting','paused','complete','errored','terminated')) OR
  (OLD.state='waiting' AND NEW.state IN ('queued','paused','terminated')) OR
  (OLD.state='paused' AND NEW.state IN ('queued','waiting','terminated')) OR
  (NEW.state='queued' AND EXISTS(SELECT 1 FROM workflow_mutation_context c WHERE c.instance_id=OLD.id
    AND c.kind='restart' AND c.creation_nonce=OLD.creation_nonce AND c.expected_generation=OLD.instance_generation
    AND c.target_generation=NEW.instance_generation AND c.operation_id=NEW.last_restart_operation_id))
) BEGIN SELECT RAISE(ABORT,'workflow durable state transition'); END;
CREATE TRIGGER workflow_v2_instance_run_guard BEFORE UPDATE ON workflow_instances
WHEN OLD.capability_version=2 AND NOT EXISTS(SELECT 1 FROM workflow_mutation_context WHERE instance_id=OLD.id) AND (
  (OLD.state='running' AND NEW.state='running' AND (NEW.run_token!=OLD.run_token
    OR NEW.run_claimed_at_ms!=OLD.run_claimed_at_ms OR NEW.run_lease_until_ms<OLD.run_lease_until_ms
    OR NEW.pause_requested<OLD.pause_requested OR NEW.yield_requested<OLD.yield_requested)) OR
  (OLD.state='running' AND NEW.state IN ('queued','waiting','paused') AND NEW.updated_at_ms<OLD.run_lease_until_ms
    AND OLD.yield_requested=0 AND OLD.pause_requested=0) OR
  (OLD.state='running' AND NEW.state IN ('complete','errored') AND NEW.updated_at_ms>=OLD.run_lease_until_ms) OR
  (NEW.state='running' AND NEW.has_activated!=1)
) BEGIN SELECT RAISE(ABORT,'workflow durable run fence'); END;
CREATE TRIGGER workflow_v2_instance_frontier_guard BEFORE UPDATE OF state ON workflow_instances
WHEN OLD.capability_version=2 AND NOT EXISTS(SELECT 1 FROM workflow_mutation_context WHERE instance_id=OLD.id) AND (
  (NEW.state!='running' AND EXISTS(SELECT 1 FROM workflow_steps WHERE instance_id=OLD.id AND state='running')) OR
  (NEW.state='complete' AND NEW.settled_step_count!=NEW.registered_step_count) OR
  (NEW.state IN ('complete','errored','terminated') AND EXISTS(SELECT 1 FROM workflow_steps WHERE instance_id=OLD.id AND state IN ('pending','waiting','retry_wait'))) OR
  (NEW.state='waiting' AND (NEW.next_wake_at_ms IS NULL OR EXISTS(SELECT 1 FROM workflow_steps WHERE instance_id=OLD.id AND state='pending')))
) BEGIN SELECT RAISE(ABORT,'workflow durable unsettled frontier'); END;
CREATE TRIGGER workflow_v2_instance_accounting_guard BEFORE UPDATE ON workflow_instances
WHEN OLD.capability_version=2 AND EXISTS(SELECT 1 FROM workflow_v2_accounting a WHERE a.id=OLD.id AND (
  NEW.registered_step_count!=a.registered OR NEW.settled_step_count!=a.settled OR NEW.completed_step_count!=a.completed
  OR NEW.event_count!=a.event_count OR NEW.event_bytes!=a.event_bytes OR NEW.next_wake_at_ms IS NOT a.next_wake
  OR NEW.state_bytes!=256+length(NEW.input_json)+coalesce(length(NEW.output_json),0)+coalesce(length(NEW.error_json),0)
    +length(CAST(NEW.definition_name AS BLOB))+length(CAST(NEW.external_instance_id AS BLOB))+length(CAST(NEW.class_name AS BLOB))+a.history_bytes))
BEGIN SELECT RAISE(ABORT,'workflow durable accounting'); END;
CREATE TRIGGER workflow_v2_instance_delete_guard BEFORE DELETE ON workflow_instances
WHEN OLD.capability_version=2 AND NOT EXISTS(SELECT 1 FROM workflow_mutation_context c WHERE c.instance_id=OLD.id
  AND c.kind='purge' AND c.creation_nonce=OLD.creation_nonce AND c.expected_generation=OLD.instance_generation
  AND OLD.expires_at_ms<=c.authorized_at_ms AND OLD.state IN ('complete','errored','terminated') AND OLD.run_token IS NULL)
BEGIN SELECT RAISE(ABORT,'workflow deletion requires exact purge'); END;

CREATE TRIGGER workflow_step_capability_guard BEFORE INSERT ON workflow_steps
WHEN (NEW.config_sha256 IS NULL) IS NOT (SELECT capability_version=1 FROM workflow_instances WHERE id=NEW.instance_id)
BEGIN SELECT RAISE(ABORT,'workflow step capability'); END;
CREATE TRIGGER workflow_step_extra_identity_guard BEFORE UPDATE OF config_sha256,batch_first_ordinal,batch_size,
  dependency_count,event_buffer_ceiling ON workflow_steps
BEGIN SELECT RAISE(ABORT,'workflow durable descriptor is immutable'); END;
CREATE TRIGGER workflow_v2_step_insert_guard BEFORE INSERT ON workflow_steps
WHEN NEW.config_sha256 IS NOT NULL
BEGIN
  SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=NEW.instance_id
    AND i.capability_version=2 AND i.instance_generation=NEW.instance_generation AND i.state='running'
    AND i.run_lease_until_ms>NEW.updated_at_ms AND i.pause_requested=0 AND i.yield_requested=0
    AND i.registered_step_count=NEW.ordinal)
    OR NEW.attempt!=0 OR NEW.state!=CASE WHEN NEW.kind='do' THEN 'pending' ELSE 'waiting' END
    OR NEW.name_count!=1+(SELECT COUNT(*) FROM workflow_steps s WHERE s.instance_id=NEW.instance_id AND s.kind=NEW.kind AND s.name=NEW.name)
    OR EXISTS(SELECT 1 FROM workflow_steps s WHERE s.instance_id=NEW.instance_id AND s.ordinal<NEW.batch_first_ordinal AND s.state NOT IN ('complete','failed'))
    OR NEW.dependency_count!=coalesce((SELECT batch_size FROM workflow_steps s WHERE s.instance_id=NEW.instance_id AND s.ordinal=NEW.batch_first_ordinal-1),0)
    OR (NEW.ordinal!=NEW.batch_first_ordinal AND NOT EXISTS(SELECT 1 FROM workflow_steps s WHERE s.instance_id=NEW.instance_id
      AND s.ordinal=NEW.batch_first_ordinal AND s.kind='do' AND s.batch_size=NEW.batch_size AND s.dependency_count=NEW.dependency_count))
    THEN RAISE(ABORT,'workflow durable registration frontier') END;
  SELECT CASE WHEN (NEW.kind='wait_event' AND NEW.event_buffer_ceiling!=(SELECT next_event_seq-1 FROM workflow_instances WHERE id=NEW.instance_id))
    OR (NEW.kind='sleep' AND NEW.due_at_ms IS NOT NEW.started_at_ms+json_extract(CAST(NEW.config_json AS TEXT),'$.durationMs'))
    OR (NEW.kind='sleep_until' AND NEW.due_at_ms IS NOT json_extract(CAST(NEW.config_json AS TEXT),'$.timestampMs'))
    OR (NEW.kind='wait_event' AND NEW.due_at_ms IS NOT NEW.started_at_ms+json_extract(CAST(NEW.config_json AS TEXT),'$.timeoutMs'))
    THEN RAISE(ABORT,'workflow durable registration deadline') END;
END;
CREATE TRIGGER workflow_v2_step_identity_guard BEFORE UPDATE OF instance_id,instance_generation,ordinal,name,name_count,
  kind,config_json,descriptor_sha256,started_at_ms ON workflow_steps WHEN OLD.config_sha256 IS NOT NULL
BEGIN SELECT RAISE(ABORT,'workflow durable step identity is immutable'); END;
CREATE TRIGGER workflow_v2_step_terminal_guard BEFORE UPDATE ON workflow_steps
WHEN OLD.config_sha256 IS NOT NULL AND OLD.state IN ('complete','failed','cancelled')
BEGIN SELECT RAISE(ABORT,'workflow settled result is immutable'); END;
CREATE TRIGGER workflow_v2_step_attempt_guard BEFORE UPDATE ON workflow_steps
WHEN OLD.config_sha256 IS NOT NULL AND (NEW.attempt!=OLD.attempt
  OR NEW.attempt_started_at_ms IS NOT OLD.attempt_started_at_ms OR NEW.attempt_deadline_at_ms IS NOT OLD.attempt_deadline_at_ms)
  AND NOT (NEW.state='running' AND NEW.kind='do' AND NEW.attempt=OLD.attempt+1
    AND ((OLD.state='pending' AND OLD.attempt=0) OR (OLD.state='retry_wait' AND OLD.due_at_ms<=NEW.updated_at_ms))
    AND NEW.attempt_started_at_ms=NEW.updated_at_ms
    AND NEW.attempt_deadline_at_ms=NEW.attempt_started_at_ms+json_extract(CAST(NEW.config_json AS TEXT),'$.timeout')
    AND NEW.attempt<=1+json_extract(CAST(NEW.config_json AS TEXT),'$.retries.limit'))
BEGIN SELECT RAISE(ABORT,'workflow durable business attempt'); END;
CREATE TRIGGER workflow_v2_step_transition_guard BEFORE UPDATE ON workflow_steps
WHEN OLD.config_sha256 IS NOT NULL AND NOT (
  (OLD.state IN ('pending','retry_wait') AND NEW.state='running' AND NEW.attempt_deadline_at_ms>NEW.updated_at_ms
    AND (OLD.state!='retry_wait' OR (OLD.due_at_ms<=NEW.updated_at_ms AND NEW.attempt=OLD.attempt+1))
    AND (SELECT COUNT(*) FROM workflow_steps s WHERE s.instance_id=OLD.instance_id AND s.batch_first_ordinal=OLD.batch_first_ordinal)=OLD.batch_size
    AND (SELECT COUNT(*) FROM workflow_step_dependencies d WHERE d.instance_id=OLD.instance_id AND d.child_ordinal=OLD.ordinal)=OLD.dependency_count
    AND EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=OLD.instance_id AND i.state='running'
      AND i.run_token=NEW.run_token AND i.run_lease_until_ms>NEW.updated_at_ms AND i.pause_requested=0 AND i.yield_requested=0)) OR
  (OLD.state='running' AND NEW.state IN ('complete','failed','retry_wait')
    AND (NEW.state!='complete' OR NEW.updated_at_ms<OLD.attempt_deadline_at_ms)
    AND EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=OLD.instance_id AND i.state='running'
      AND i.run_token=OLD.run_token AND i.run_lease_until_ms>NEW.updated_at_ms)) OR
  (OLD.state='running' AND NEW.state='pending' AND EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=OLD.instance_id
    AND i.state='running' AND i.run_token=OLD.run_token AND i.run_lease_until_ms<=NEW.updated_at_ms)) OR
  (OLD.state='pending' AND OLD.attempt>0 AND OLD.attempt_deadline_at_ms<=NEW.updated_at_ms
    AND NEW.state IN ('failed','retry_wait') AND EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=OLD.instance_id
      AND i.state IN ('queued','running','waiting','paused'))) OR
  (OLD.state='waiting' AND NEW.state IN ('complete','failed') AND EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=OLD.instance_id
    AND i.state IN ('queued','running','waiting','paused'))) OR
  (NEW.state='cancelled' AND OLD.state IN ('pending','running','waiting','retry_wait') AND EXISTS(
    SELECT 1 FROM workflow_instances i WHERE i.id=OLD.instance_id AND i.state IN ('queued','running','waiting','paused')))
) BEGIN SELECT RAISE(ABORT,'workflow durable step fence'); END;
CREATE TRIGGER workflow_v2_wait_settlement_guard BEFORE UPDATE ON workflow_steps
WHEN OLD.config_sha256 IS NOT NULL AND OLD.state='waiting'
BEGIN
  SELECT CASE WHEN NEW.state='complete' AND OLD.kind IN ('sleep','sleep_until') AND OLD.due_at_ms>NEW.updated_at_ms
    THEN RAISE(ABORT,'workflow sleep deadline not due') END;
  SELECT CASE WHEN NEW.state='complete' AND OLD.kind='wait_event' AND NOT EXISTS(
    SELECT 1 FROM workflow_events e WHERE e.instance_id=OLD.instance_id AND e.instance_generation=OLD.instance_generation
      AND e.event_seq=NEW.consumed_event_seq AND e.type=json_extract(CAST(OLD.config_json AS TEXT),'$.type')
      AND (e.accepted_at_ms<OLD.due_at_ms OR e.event_seq<=OLD.event_buffer_ceiling)
      AND NOT EXISTS(SELECT 1 FROM workflow_events p WHERE p.instance_id=e.instance_id AND p.instance_generation=e.instance_generation
        AND p.type=e.type AND p.event_seq<e.event_seq)
      AND NOT EXISTS(SELECT 1 FROM workflow_steps s WHERE s.instance_id=OLD.instance_id AND s.consumed_event_seq=e.event_seq)
      AND json_extract(CAST(NEW.output_json AS TEXT),'$.type')=e.type
      AND json_extract(CAST(NEW.output_json AS TEXT),'$.timestampMs')=e.accepted_at_ms
      AND json_extract(CAST(NEW.output_json AS TEXT),'$.payload') IS json_extract(CAST(e.payload_json AS TEXT),'$'))
    THEN RAISE(ABORT,'workflow event is not eligible') END;
  SELECT CASE WHEN NEW.state='failed' AND (OLD.kind!='wait_event' OR OLD.due_at_ms>NEW.updated_at_ms
    OR NEW.error_code!='WORKFLOW_EVENT_TIMEOUT' OR EXISTS(SELECT 1 FROM workflow_events e WHERE e.instance_id=OLD.instance_id
      AND e.instance_generation=OLD.instance_generation AND e.type=json_extract(CAST(OLD.config_json AS TEXT),'$.type')
      AND (e.accepted_at_ms<OLD.due_at_ms OR e.event_seq<=OLD.event_buffer_ceiling)))
    THEN RAISE(ABORT,'workflow event timeout arbitration') END;
END;
CREATE TRIGGER workflow_v2_step_delete_guard BEFORE DELETE ON workflow_steps
WHEN OLD.config_sha256 IS NOT NULL AND NOT EXISTS(SELECT 1 FROM workflow_mutation_context c JOIN workflow_instances i ON i.id=c.instance_id
  WHERE c.instance_id=OLD.instance_id AND c.expected_generation=OLD.instance_generation AND c.kind IN ('restart','purge')
    AND c.creation_nonce=i.creation_nonce AND c.expected_generation=i.instance_generation)
BEGIN SELECT RAISE(ABORT,'workflow step history requires exact operation'); END;

CREATE TRIGGER workflow_dependency_insert_guard BEFORE INSERT ON workflow_step_dependencies
WHEN NOT EXISTS(SELECT 1 FROM workflow_steps child JOIN workflow_steps parent ON parent.instance_id=child.instance_id
  AND parent.instance_generation=child.instance_generation JOIN workflow_instances i ON i.id=child.instance_id
  WHERE child.instance_id=NEW.instance_id AND child.instance_generation=NEW.instance_generation
    AND child.ordinal=NEW.child_ordinal AND parent.ordinal=NEW.parent_ordinal AND child.config_sha256 IS NOT NULL
    AND child.state IN ('pending','waiting') AND i.state='running' AND i.pause_requested=0 AND i.yield_requested=0
    AND parent.state IN ('complete','failed') AND parent.ordinal<child.batch_first_ordinal
    AND parent.batch_first_ordinal=(SELECT batch_first_ordinal FROM workflow_steps WHERE instance_id=child.instance_id AND ordinal=child.batch_first_ordinal-1)
    AND child.dependency_count>(SELECT COUNT(*) FROM workflow_step_dependencies WHERE instance_id=child.instance_id AND child_ordinal=child.ordinal))
BEGIN SELECT RAISE(ABORT,'workflow dependency frontier'); END;
CREATE TRIGGER workflow_dependency_immutable BEFORE UPDATE ON workflow_step_dependencies
BEGIN SELECT RAISE(ABORT,'workflow dependency is immutable'); END;
CREATE TRIGGER workflow_dependency_delete_guard BEFORE DELETE ON workflow_step_dependencies
WHEN NOT EXISTS(SELECT 1 FROM workflow_mutation_context c JOIN workflow_instances i ON i.id=c.instance_id
  WHERE c.instance_id=OLD.instance_id AND c.expected_generation=OLD.instance_generation AND c.kind IN ('restart','purge')
    AND c.creation_nonce=i.creation_nonce AND c.expected_generation=i.instance_generation)
BEGIN SELECT RAISE(ABORT,'workflow dependency history requires exact operation'); END;

CREATE TRIGGER workflow_event_insert_guard BEFORE INSERT ON workflow_events
WHEN NEW.type GLOB '*[^A-Za-z0-9_-]*' OR NEW.type GLOB '-*' OR NOT json_valid(CAST(NEW.payload_json AS TEXT))
  OR NOT EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=NEW.instance_id AND i.instance_generation=NEW.instance_generation
    AND i.capability_version=2 AND i.state IN ('queued','running','waiting','paused') AND i.next_event_seq=NEW.event_seq
    AND i.next_event_seq<9223372036854775807)
BEGIN SELECT RAISE(ABORT,'workflow event intake fence'); END;
CREATE TRIGGER workflow_event_immutable BEFORE UPDATE ON workflow_events
BEGIN SELECT RAISE(ABORT,'workflow event is immutable'); END;
CREATE TRIGGER workflow_event_delete_guard BEFORE DELETE ON workflow_events
WHEN NOT EXISTS(SELECT 1 FROM workflow_steps s WHERE s.instance_id=OLD.instance_id AND s.instance_generation=OLD.instance_generation
    AND s.kind='wait_event' AND s.state='complete' AND s.consumed_event_seq=OLD.event_seq)
  AND NOT EXISTS(SELECT 1 FROM workflow_mutation_context c JOIN workflow_instances i ON i.id=c.instance_id
    WHERE c.instance_id=OLD.instance_id AND c.expected_generation=OLD.instance_generation AND c.kind IN ('restart','purge')
      AND c.creation_nonce=i.creation_nonce AND c.expected_generation=i.instance_generation)
BEGIN SELECT RAISE(ABORT,'workflow event deletion requires consumption or operation'); END;
CREATE TRIGGER workflow_v2_event_sequence_guard BEFORE UPDATE OF next_event_seq ON workflow_instances
WHEN OLD.capability_version=2 AND NEW.next_event_seq!=OLD.next_event_seq
  AND NOT (NEW.next_event_seq=OLD.next_event_seq+1 AND EXISTS(SELECT 1 FROM workflow_events e WHERE e.instance_id=OLD.id AND e.event_seq=OLD.next_event_seq))
  AND NOT EXISTS(SELECT 1 FROM workflow_mutation_context c WHERE c.instance_id=OLD.id AND c.kind='restart'
    AND c.creation_nonce=OLD.creation_nonce AND c.expected_generation=OLD.instance_generation AND c.target_generation=NEW.instance_generation AND NEW.next_event_seq=1)
BEGIN SELECT RAISE(ABORT,'workflow event sequence is monotonic'); END;

-- Every history mutation reconciles the exact affected instance in the same transaction.
CREATE TRIGGER workflow_v2_steps_insert_accounting AFTER INSERT ON workflow_steps
WHEN (SELECT capability_version FROM workflow_instances WHERE id=NEW.instance_id)=2
BEGIN
  UPDATE workflow_instances SET
    registered_step_count=(SELECT registered FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    settled_step_count=(SELECT settled FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    completed_step_count=(SELECT completed FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    event_count=(SELECT event_count FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    event_bytes=(SELECT event_bytes FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    next_wake_at_ms=(SELECT next_wake FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    state_bytes=256+length(input_json)+coalesce(length(output_json),0)+coalesce(length(error_json),0)
      +length(CAST(definition_name AS BLOB))+length(CAST(external_instance_id AS BLOB))+length(CAST(class_name AS BLOB))
      +(SELECT history_bytes FROM workflow_v2_accounting WHERE id=NEW.instance_id)
    WHERE id=NEW.instance_id AND capability_version=2;
END;
CREATE TRIGGER workflow_v2_steps_update_accounting AFTER UPDATE ON workflow_steps
WHEN (SELECT capability_version FROM workflow_instances WHERE id=NEW.instance_id)=2
BEGIN
  UPDATE workflow_instances SET
    registered_step_count=(SELECT registered FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    settled_step_count=(SELECT settled FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    completed_step_count=(SELECT completed FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    event_count=(SELECT event_count FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    event_bytes=(SELECT event_bytes FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    next_wake_at_ms=(SELECT next_wake FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    state_bytes=256+length(input_json)+coalesce(length(output_json),0)+coalesce(length(error_json),0)
      +length(CAST(definition_name AS BLOB))+length(CAST(external_instance_id AS BLOB))+length(CAST(class_name AS BLOB))
      +(SELECT history_bytes FROM workflow_v2_accounting WHERE id=NEW.instance_id)
    WHERE id=NEW.instance_id AND capability_version=2;
END;
CREATE TRIGGER workflow_v2_steps_delete_accounting AFTER DELETE ON workflow_steps
WHEN (SELECT capability_version FROM workflow_instances WHERE id=OLD.instance_id)=2
BEGIN
  UPDATE workflow_instances SET
    registered_step_count=(SELECT registered FROM workflow_v2_accounting WHERE id=OLD.instance_id),
    settled_step_count=(SELECT settled FROM workflow_v2_accounting WHERE id=OLD.instance_id),
    completed_step_count=(SELECT completed FROM workflow_v2_accounting WHERE id=OLD.instance_id),
    event_count=(SELECT event_count FROM workflow_v2_accounting WHERE id=OLD.instance_id),
    event_bytes=(SELECT event_bytes FROM workflow_v2_accounting WHERE id=OLD.instance_id),
    next_wake_at_ms=(SELECT next_wake FROM workflow_v2_accounting WHERE id=OLD.instance_id),
    state_bytes=256+length(input_json)+coalesce(length(output_json),0)+coalesce(length(error_json),0)
      +length(CAST(definition_name AS BLOB))+length(CAST(external_instance_id AS BLOB))+length(CAST(class_name AS BLOB))
      +(SELECT history_bytes FROM workflow_v2_accounting WHERE id=OLD.instance_id)
    WHERE id=OLD.instance_id AND capability_version=2;
END;
CREATE TRIGGER workflow_v2_step_dependencies_insert_accounting AFTER INSERT ON workflow_step_dependencies
WHEN (SELECT capability_version FROM workflow_instances WHERE id=NEW.instance_id)=2
BEGIN
  UPDATE workflow_instances SET
    registered_step_count=(SELECT registered FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    settled_step_count=(SELECT settled FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    completed_step_count=(SELECT completed FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    event_count=(SELECT event_count FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    event_bytes=(SELECT event_bytes FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    next_wake_at_ms=(SELECT next_wake FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    state_bytes=256+length(input_json)+coalesce(length(output_json),0)+coalesce(length(error_json),0)
      +length(CAST(definition_name AS BLOB))+length(CAST(external_instance_id AS BLOB))+length(CAST(class_name AS BLOB))
      +(SELECT history_bytes FROM workflow_v2_accounting WHERE id=NEW.instance_id)
    WHERE id=NEW.instance_id AND capability_version=2;
END;
CREATE TRIGGER workflow_v2_step_dependencies_delete_accounting AFTER DELETE ON workflow_step_dependencies
WHEN (SELECT capability_version FROM workflow_instances WHERE id=OLD.instance_id)=2
BEGIN
  UPDATE workflow_instances SET
    registered_step_count=(SELECT registered FROM workflow_v2_accounting WHERE id=OLD.instance_id),
    settled_step_count=(SELECT settled FROM workflow_v2_accounting WHERE id=OLD.instance_id),
    completed_step_count=(SELECT completed FROM workflow_v2_accounting WHERE id=OLD.instance_id),
    event_count=(SELECT event_count FROM workflow_v2_accounting WHERE id=OLD.instance_id),
    event_bytes=(SELECT event_bytes FROM workflow_v2_accounting WHERE id=OLD.instance_id),
    next_wake_at_ms=(SELECT next_wake FROM workflow_v2_accounting WHERE id=OLD.instance_id),
    state_bytes=256+length(input_json)+coalesce(length(output_json),0)+coalesce(length(error_json),0)
      +length(CAST(definition_name AS BLOB))+length(CAST(external_instance_id AS BLOB))+length(CAST(class_name AS BLOB))
      +(SELECT history_bytes FROM workflow_v2_accounting WHERE id=OLD.instance_id)
    WHERE id=OLD.instance_id AND capability_version=2;
END;
CREATE TRIGGER workflow_v2_events_insert_accounting AFTER INSERT ON workflow_events
WHEN (SELECT capability_version FROM workflow_instances WHERE id=NEW.instance_id)=2
BEGIN
  UPDATE workflow_instances SET
    registered_step_count=(SELECT registered FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    settled_step_count=(SELECT settled FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    completed_step_count=(SELECT completed FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    event_count=(SELECT event_count FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    event_bytes=(SELECT event_bytes FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    next_wake_at_ms=(SELECT next_wake FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    state_bytes=256+length(input_json)+coalesce(length(output_json),0)+coalesce(length(error_json),0)
      +length(CAST(definition_name AS BLOB))+length(CAST(external_instance_id AS BLOB))+length(CAST(class_name AS BLOB))
      +(SELECT history_bytes FROM workflow_v2_accounting WHERE id=NEW.instance_id),
    next_event_seq=next_event_seq+1
    WHERE id=NEW.instance_id AND capability_version=2;
END;
CREATE TRIGGER workflow_v2_events_delete_accounting AFTER DELETE ON workflow_events
WHEN (SELECT capability_version FROM workflow_instances WHERE id=OLD.instance_id)=2
BEGIN
  UPDATE workflow_instances SET
    registered_step_count=(SELECT registered FROM workflow_v2_accounting WHERE id=OLD.instance_id),
    settled_step_count=(SELECT settled FROM workflow_v2_accounting WHERE id=OLD.instance_id),
    completed_step_count=(SELECT completed FROM workflow_v2_accounting WHERE id=OLD.instance_id),
    event_count=(SELECT event_count FROM workflow_v2_accounting WHERE id=OLD.instance_id),
    event_bytes=(SELECT event_bytes FROM workflow_v2_accounting WHERE id=OLD.instance_id),
    next_wake_at_ms=(SELECT next_wake FROM workflow_v2_accounting WHERE id=OLD.instance_id),
    state_bytes=256+length(input_json)+coalesce(length(output_json),0)+coalesce(length(error_json),0)
      +length(CAST(definition_name AS BLOB))+length(CAST(external_instance_id AS BLOB))+length(CAST(class_name AS BLOB))
      +(SELECT history_bytes FROM workflow_v2_accounting WHERE id=OLD.instance_id)
    WHERE id=OLD.instance_id AND capability_version=2;
END;

CREATE TEMP TABLE workflow_copy_check (valid INTEGER NOT NULL CHECK(valid=1));
INSERT INTO workflow_copy_check SELECT NOT EXISTS(SELECT id,account_id,definition_id,definition_name,external_instance_id,version_id,worker_id,deployment_id,worker_code_sha256,loader_schema_version,capability_version,descriptor_sha256,class_name,creation_nonce,instance_generation,state,input_json,output_json,error_json,error_code,next_run_at_ms,run_token,run_claimed_at_ms,run_lease_until_ms,completed_step_count,state_bytes,created_at_ms,updated_at_ms,terminal_at_ms FROM workflow_instances EXCEPT SELECT * FROM saved_workflow_instances) AND NOT EXISTS(SELECT * FROM saved_workflow_instances EXCEPT SELECT id,account_id,definition_id,definition_name,external_instance_id,version_id,worker_id,deployment_id,worker_code_sha256,loader_schema_version,capability_version,descriptor_sha256,class_name,creation_nonce,instance_generation,state,input_json,output_json,error_json,error_code,next_run_at_ms,run_token,run_claimed_at_ms,run_lease_until_ms,completed_step_count,state_bytes,created_at_ms,updated_at_ms,terminal_at_ms FROM workflow_instances);
INSERT INTO workflow_copy_check SELECT NOT EXISTS(SELECT instance_id,instance_generation,ordinal,name,name_count,kind,config_json,descriptor_sha256,state,attempt,run_token,step_token,output_json,error_json,error_code,started_at_ms,updated_at_ms,completed_at_ms FROM workflow_steps EXCEPT SELECT * FROM saved_workflow_steps) AND NOT EXISTS(SELECT * FROM saved_workflow_steps EXCEPT SELECT instance_id,instance_generation,ordinal,name,name_count,kind,config_json,descriptor_sha256,state,attempt,run_token,step_token,output_json,error_json,error_code,started_at_ms,updated_at_ms,completed_at_ms FROM workflow_steps);
INSERT INTO workflow_copy_check SELECT NOT EXISTS(SELECT 1 FROM pragma_foreign_key_check);
DROP TABLE saved_workflow_steps;
DROP TABLE saved_workflow_instances;
DROP TABLE workflow_copy_check;
