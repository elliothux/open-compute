-- Numeric Workflow retention is milliseconds, including zero. Preserve all durable state and guards.

PRAGMA defer_foreign_keys=ON;

DROP TRIGGER workflow_context_delete_guard;

DROP TRIGGER workflow_context_insert_guard;

DROP TRIGGER workflow_dependency_delete_guard;

DROP TRIGGER workflow_dependency_insert_guard;

DROP TRIGGER workflow_event_delete_guard;

DROP TRIGGER workflow_event_insert_guard;

DROP TRIGGER workflow_event_sequence_guard;

DROP TRIGGER workflow_events_delete_accounting;

DROP TRIGGER workflow_events_insert_accounting;

DROP TRIGGER workflow_generation_guard;

DROP TRIGGER workflow_instance_accounting_guard;

DROP TRIGGER workflow_instance_delete_guard;

DROP TRIGGER workflow_instance_frontier_guard;

DROP TRIGGER workflow_instance_identity_guard;

DROP TRIGGER workflow_instance_insert_guard;

DROP TRIGGER workflow_instance_run_guard;

DROP TRIGGER workflow_instance_terminal_guard;

DROP TRIGGER workflow_instance_transition_guard;

DROP TRIGGER workflow_progress_insert_guard;

DROP TRIGGER workflow_progress_update_guard;

DROP TRIGGER workflow_receipt_insert_guard;

DROP TRIGGER workflow_step_delete_guard;

DROP TRIGGER workflow_step_dependencies_delete_accounting;

DROP TRIGGER workflow_step_dependencies_insert_accounting;

DROP TRIGGER workflow_step_insert_guard;

DROP TRIGGER workflow_step_transition_guard;

DROP TRIGGER workflow_steps_delete_accounting;

DROP TRIGGER workflow_steps_insert_accounting;

DROP TRIGGER workflow_steps_update_accounting;

DROP VIEW workflow_accounting;

CREATE TABLE workflow_instances_next (
  id TEXT PRIMARY KEY,
  definition_id TEXT NOT NULL,
  definition_name TEXT NOT NULL,
  external_instance_id TEXT NOT NULL,
  workflow_version_id TEXT NOT NULL,
  worker_id TEXT NOT NULL,
  worker_version_id TEXT NOT NULL,
  worker_code_sha256 BLOB NOT NULL CHECK(length(worker_code_sha256)=32),
  loader_schema_version INTEGER NOT NULL CHECK(loader_schema_version>0),
  capability_version INTEGER NOT NULL CHECK(capability_version = 1),
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256)=32),
  class_name TEXT NOT NULL,
  creation_nonce BLOB NOT NULL CHECK(length(creation_nonce)=32),
  creation_operation_id TEXT NOT NULL UNIQUE,
  creation_batch_id TEXT NOT NULL,
  instance_generation INTEGER NOT NULL CHECK(instance_generation>=1),
  state TEXT NOT NULL CHECK(state IN ('queued','running','waiting','paused','complete','errored','terminated')),
  input_json BLOB NOT NULL CHECK(length(input_json)<=1398112),
  output_json BLOB CHECK(output_json IS NULL OR length(output_json)<=1398112),
  error_json BLOB CHECK(error_json IS NULL OR length(error_json)<=8192),
  error_code TEXT,
  next_run_at_ms INTEGER,
  run_token BLOB,
  run_claimed_at_ms INTEGER,
  run_lease_until_ms INTEGER,
  completed_step_count INTEGER NOT NULL DEFAULT 0 CHECK(completed_step_count BETWEEN 0 AND 1024),
  state_bytes INTEGER NOT NULL CHECK(state_bytes>=0),
  trigger_cron TEXT CHECK(trigger_cron IS NULL OR length(trigger_cron) BETWEEN 1 AND 256),
  trigger_scheduled_time_ms INTEGER CHECK(trigger_scheduled_time_ms IS NULL OR trigger_scheduled_time_ms >= 0),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  terminal_at_ms INTEGER,
  pause_requested INTEGER NOT NULL DEFAULT 0 CHECK(pause_requested IN (0,1)),
  yield_requested INTEGER NOT NULL DEFAULT 0 CHECK(yield_requested IN (0,1)),
  rollback_requested INTEGER NOT NULL DEFAULT 0 CHECK(rollback_requested IN (0,1)),
  next_wake_at_ms INTEGER,
  registered_step_count INTEGER NOT NULL DEFAULT 0 CHECK(registered_step_count BETWEEN 0 AND 1024),
  settled_step_count INTEGER NOT NULL DEFAULT 0 CHECK(settled_step_count BETWEEN 0 AND 1024),
  success_retention_ms INTEGER NOT NULL,
  error_retention_ms INTEGER NOT NULL,
  expires_at_ms INTEGER,
  last_restart_operation_id TEXT,
  last_restart_from_name TEXT,
  last_restart_from_count INTEGER,
  last_restart_from_kind TEXT,
  last_restart_target_ordinal INTEGER,
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
  CHECK((trigger_cron IS NULL)=(trigger_scheduled_time_ms IS NULL)),
  CHECK((state='errored')=(error_json IS NOT NULL)),
  CHECK((state='errored')=(error_code IS NOT NULL)),
  CHECK(state='running' OR (pause_requested=0 AND yield_requested=0)),
  CHECK(state NOT IN ('complete','errored','terminated') OR rollback_requested=0),
  CHECK(success_retention_ms BETWEEN 0 AND 31536000000),
  CHECK(error_retention_ms BETWEEN 0 AND 31536000000),
  CHECK(completed_step_count<=settled_step_count AND settled_step_count<=registered_step_count),
  CHECK((terminal_at_ms IS NULL AND expires_at_ms IS NULL) OR
    (terminal_at_ms IS NOT NULL AND expires_at_ms IS NOT NULL AND expires_at_ms<=9007199254740991
      AND expires_at_ms=terminal_at_ms+CASE WHEN state='complete' THEN success_retention_ms ELSE error_retention_ms END)),
  CHECK((last_restart_operation_id IS NULL AND last_restart_from_name IS NULL AND last_restart_from_count IS NULL
      AND last_restart_from_kind IS NULL AND last_restart_target_ordinal IS NULL)
    OR (last_restart_operation_id IS NOT NULL AND ((last_restart_from_name IS NULL AND last_restart_from_count IS NULL
        AND last_restart_from_kind IS NULL AND last_restart_target_ordinal IS NULL)
      OR (length(CAST(last_restart_from_name AS BLOB)) BETWEEN 1 AND 256
        AND last_restart_from_count BETWEEN 1 AND 1024
        AND (last_restart_from_kind IS NULL OR last_restart_from_kind IN ('do','sleep','waitForEvent'))
        AND last_restart_target_ordinal BETWEEN 0 AND 1023))))
) STRICT;

INSERT INTO workflow_instances_next (id,definition_id,definition_name,external_instance_id,workflow_version_id,worker_id,worker_version_id,worker_code_sha256,loader_schema_version,capability_version,descriptor_sha256,class_name,creation_nonce,creation_operation_id,creation_batch_id,instance_generation,state,input_json,output_json,error_json,error_code,next_run_at_ms,run_token,run_claimed_at_ms,run_lease_until_ms,completed_step_count,state_bytes,trigger_cron,trigger_scheduled_time_ms,created_at_ms,updated_at_ms,terminal_at_ms,pause_requested,yield_requested,rollback_requested,next_wake_at_ms,registered_step_count,settled_step_count,success_retention_ms,error_retention_ms,expires_at_ms,last_restart_operation_id,last_restart_from_name,last_restart_from_count,last_restart_from_kind,last_restart_target_ordinal,event_count,event_bytes,next_event_seq,has_activated)
SELECT id,definition_id,definition_name,external_instance_id,workflow_version_id,worker_id,worker_version_id,worker_code_sha256,loader_schema_version,capability_version,descriptor_sha256,class_name,creation_nonce,creation_operation_id,creation_batch_id,instance_generation,state,input_json,output_json,error_json,error_code,next_run_at_ms,run_token,run_claimed_at_ms,run_lease_until_ms,completed_step_count,state_bytes,trigger_cron,trigger_scheduled_time_ms,created_at_ms,updated_at_ms,terminal_at_ms,pause_requested,yield_requested,rollback_requested,next_wake_at_ms,registered_step_count,settled_step_count,success_retention_ms,error_retention_ms,expires_at_ms,last_restart_operation_id,last_restart_from_name,last_restart_from_count,last_restart_from_kind,last_restart_target_ordinal,event_count,event_bytes,next_event_seq,has_activated FROM workflow_instances;

DROP TABLE workflow_instances;

ALTER TABLE workflow_instances_next RENAME TO workflow_instances;

CREATE INDEX workflow_instance_creation_batch ON workflow_instances(creation_batch_id);

CREATE INDEX workflow_instances_definition ON workflow_instances(definition_id,state);

CREATE INDEX workflow_instances_due ON workflow_instances(next_run_at_ms,created_at_ms,id) WHERE state='queued';

CREATE INDEX workflow_instances_expired ON workflow_instances(run_lease_until_ms,id) WHERE state='running';

CREATE INDEX workflow_instances_fair ON workflow_instances(has_activated,next_run_at_ms,created_at_ms,id)
  WHERE state='queued';

CREATE INDEX workflow_instances_retention ON workflow_instances(expires_at_ms,id) WHERE capability_version=1 AND state IN ('complete','errored','terminated');

CREATE INDEX workflow_instances_waiting ON workflow_instances(next_wake_at_ms,id) WHERE state='waiting';

CREATE VIEW workflow_accounting AS SELECT i.id,
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
    WHEN s.state='delay_pending' THEN s.updated_at_ms
    WHEN s.state IN ('waiting','retry_wait') THEN s.due_at_ms END) FROM workflow_steps s WHERE s.instance_id=i.id) AS next_wake
  FROM workflow_instances i WHERE i.capability_version=1
/* workflow_accounting(id,registered,settled,completed,event_count,event_bytes,history_bytes,next_wake) */;

CREATE TRIGGER workflow_context_delete_guard BEFORE DELETE ON workflow_mutation_context
WHEN NOT (
  (OLD.kind='restart' AND EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=OLD.instance_id
    AND i.creation_nonce=OLD.creation_nonce AND i.instance_generation=OLD.target_generation
    AND i.last_restart_operation_id=OLD.operation_id AND i.last_restart_from_name IS OLD.restart_from_name
    AND i.last_restart_from_count IS OLD.restart_from_count AND i.last_restart_from_kind IS OLD.restart_from_kind
    AND i.last_restart_target_ordinal IS OLD.restart_target_ordinal)) OR
  (OLD.kind='purge' AND EXISTS(SELECT 1 FROM workflow_gc_receipts r WHERE r.operation_id=OLD.operation_id
    AND r.instance_id=OLD.instance_id AND r.creation_nonce=OLD.creation_nonce AND r.instance_generation=OLD.expected_generation)) OR
  (OLD.kind='acknowledge_purge' AND NOT EXISTS(SELECT 1 FROM workflow_gc_receipts WHERE operation_id=OLD.operation_id))
) BEGIN SELECT RAISE(ABORT,'workflow operation did not commit'); END;

CREATE TRIGGER workflow_context_insert_guard BEFORE INSERT ON workflow_mutation_context
WHEN NOT (
  (NEW.kind IN ('restart','purge') AND EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=NEW.instance_id
    AND i.capability_version=1 AND i.creation_nonce=NEW.creation_nonce AND i.instance_generation=NEW.expected_generation
    AND ((NEW.kind='restart' AND (i.expires_at_ms IS NULL OR i.expires_at_ms>NEW.authorized_at_ms)
      AND NEW.restart_next_event_seq=CASE WHEN NEW.restart_from_name IS NULL THEN 1 ELSE i.next_event_seq END
      AND ((NEW.restart_from_name IS NULL AND NEW.restart_target_ordinal IS NULL AND NEW.restart_retain_step_count=0)
        OR (NEW.restart_from_name IS NOT NULL AND EXISTS(SELECT 1 FROM workflow_steps selected
          WHERE selected.instance_id=i.id AND selected.instance_generation=i.instance_generation
            AND selected.ordinal=NEW.restart_target_ordinal AND selected.name=NEW.restart_from_name
            AND selected.name_count=NEW.restart_from_count
            AND (NEW.restart_from_kind IS NULL OR (NEW.restart_from_kind='do' AND selected.kind='do')
              OR (NEW.restart_from_kind='sleep' AND selected.kind IN ('sleep','sleep_until'))
              OR (NEW.restart_from_kind='waitForEvent' AND selected.kind='wait_event'))
            AND NEW.restart_retain_step_count=selected.batch_first_ordinal+selected.batch_size)
          AND 1=(SELECT COUNT(*) FROM workflow_steps selected WHERE selected.instance_id=i.id
            AND selected.instance_generation=i.instance_generation AND selected.name=NEW.restart_from_name
            AND selected.name_count=NEW.restart_from_count
            AND (NEW.restart_from_kind IS NULL OR (NEW.restart_from_kind='do' AND selected.kind='do')
              OR (NEW.restart_from_kind='sleep' AND selected.kind IN ('sleep','sleep_until'))
              OR (NEW.restart_from_kind='waitForEvent' AND selected.kind='wait_event')))
          AND NEW.restart_target_ordinal=(SELECT ordinal FROM workflow_steps selected WHERE selected.instance_id=i.id
            AND selected.instance_generation=i.instance_generation AND selected.name=NEW.restart_from_name
            AND selected.name_count=NEW.restart_from_count
            AND (NEW.restart_from_kind IS NULL OR (NEW.restart_from_kind='do' AND selected.kind='do')
              OR (NEW.restart_from_kind='sleep' AND selected.kind IN ('sleep','sleep_until'))
              OR (NEW.restart_from_kind='waitForEvent' AND selected.kind='wait_event')))
          AND NEW.restart_target_ordinal=(SELECT COUNT(*) FROM workflow_steps prefix WHERE prefix.instance_id=i.id
            AND prefix.instance_generation=i.instance_generation AND prefix.ordinal<NEW.restart_target_ordinal
            AND prefix.state='complete')
          AND NEW.restart_retain_step_count=(SELECT COUNT(*) FROM workflow_steps retained WHERE retained.instance_id=i.id
            AND retained.instance_generation=i.instance_generation AND retained.ordinal<NEW.restart_retain_step_count))))
      OR (NEW.kind='purge' AND i.state IN ('complete','errored','terminated') AND i.run_token IS NULL)))) OR
  (NEW.kind='acknowledge_purge' AND EXISTS(SELECT 1 FROM workflow_gc_receipts r WHERE r.operation_id=NEW.operation_id
    AND r.instance_id=NEW.instance_id AND r.creation_nonce=NEW.creation_nonce AND r.instance_generation=NEW.expected_generation))
) BEGIN SELECT RAISE(ABORT,'workflow operation context identity'); END;

CREATE TRIGGER workflow_dependency_delete_guard BEFORE DELETE ON workflow_step_dependencies
WHEN NOT EXISTS(SELECT 1 FROM workflow_mutation_context c JOIN workflow_instances i ON i.id=c.instance_id
  WHERE c.instance_id=OLD.instance_id AND c.expected_generation=OLD.instance_generation AND c.kind IN ('restart','purge')
    AND c.creation_nonce=i.creation_nonce AND c.expected_generation=i.instance_generation)
BEGIN SELECT RAISE(ABORT,'workflow dependency history requires exact operation'); END;

CREATE TRIGGER workflow_dependency_insert_guard BEFORE INSERT ON workflow_step_dependencies
WHEN NOT EXISTS(SELECT 1 FROM workflow_mutation_context c WHERE c.instance_id=NEW.instance_id AND c.kind='restart'
    AND c.target_generation=NEW.instance_generation AND NEW.child_ordinal<c.restart_retain_step_count
    AND NEW.parent_ordinal<NEW.child_ordinal)
AND NOT EXISTS(SELECT 1 FROM workflow_steps child JOIN workflow_steps parent ON parent.instance_id=child.instance_id
  AND parent.instance_generation=child.instance_generation JOIN workflow_instances i ON i.id=child.instance_id
  WHERE child.instance_id=NEW.instance_id AND child.instance_generation=NEW.instance_generation
    AND child.ordinal=NEW.child_ordinal AND parent.ordinal=NEW.parent_ordinal AND child.config_sha256 IS NOT NULL
    AND child.state IN ('pending','waiting') AND i.state='running' AND i.pause_requested=0 AND i.yield_requested=0
    AND parent.state IN ('complete','failed') AND parent.ordinal<child.batch_first_ordinal
    AND child.dependency_count>(SELECT COUNT(*) FROM workflow_step_dependencies WHERE instance_id=child.instance_id AND child_ordinal=child.ordinal))
BEGIN SELECT RAISE(ABORT,'workflow dependency frontier'); END;

CREATE TRIGGER workflow_event_delete_guard BEFORE DELETE ON workflow_events
WHEN NOT EXISTS(SELECT 1 FROM workflow_steps s WHERE s.instance_id=OLD.instance_id AND s.instance_generation=OLD.instance_generation
    AND s.kind='wait_event' AND s.state='complete' AND s.consumed_event_seq=OLD.event_seq)
  AND NOT EXISTS(SELECT 1 FROM workflow_mutation_context c JOIN workflow_instances i ON i.id=c.instance_id
    WHERE c.instance_id=OLD.instance_id AND c.expected_generation=OLD.instance_generation AND c.kind IN ('restart','purge')
      AND c.creation_nonce=i.creation_nonce AND c.expected_generation=i.instance_generation)
BEGIN SELECT RAISE(ABORT,'workflow event deletion requires consumption or operation'); END;

CREATE TRIGGER workflow_event_insert_guard BEFORE INSERT ON workflow_events
WHEN NEW.type GLOB '*[^A-Za-z0-9_-]*' OR NEW.type GLOB '-*'
  OR NOT EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=NEW.instance_id AND i.instance_generation=NEW.instance_generation
    AND i.capability_version=1 AND i.state IN ('queued','running','waiting','paused') AND i.next_event_seq=NEW.event_seq
    AND i.next_event_seq<9223372036854775807)
BEGIN SELECT RAISE(ABORT,'workflow event intake fence'); END;

CREATE TRIGGER workflow_event_sequence_guard BEFORE UPDATE OF next_event_seq ON workflow_instances
WHEN OLD.capability_version=1 AND NEW.next_event_seq!=OLD.next_event_seq
  AND NOT (NEW.next_event_seq=OLD.next_event_seq+1 AND EXISTS(SELECT 1 FROM workflow_events e WHERE e.instance_id=OLD.id AND e.event_seq=OLD.next_event_seq))
  AND NOT EXISTS(SELECT 1 FROM workflow_mutation_context c WHERE c.instance_id=OLD.id AND c.kind='restart'
    AND c.creation_nonce=OLD.creation_nonce AND c.expected_generation=OLD.instance_generation
    AND c.target_generation=NEW.instance_generation AND NEW.next_event_seq=c.restart_next_event_seq)
BEGIN SELECT RAISE(ABORT,'workflow event sequence is monotonic'); END;

CREATE TRIGGER workflow_events_delete_accounting AFTER DELETE ON workflow_events
WHEN (SELECT capability_version FROM workflow_instances WHERE id=OLD.instance_id)=1
BEGIN
  UPDATE workflow_instances SET
    registered_step_count=(SELECT registered FROM workflow_accounting WHERE id=OLD.instance_id),
    settled_step_count=(SELECT settled FROM workflow_accounting WHERE id=OLD.instance_id),
    completed_step_count=(SELECT completed FROM workflow_accounting WHERE id=OLD.instance_id),
    event_count=(SELECT event_count FROM workflow_accounting WHERE id=OLD.instance_id),
    event_bytes=(SELECT event_bytes FROM workflow_accounting WHERE id=OLD.instance_id),
    next_wake_at_ms=(SELECT next_wake FROM workflow_accounting WHERE id=OLD.instance_id),
    state_bytes=256+length(input_json)+coalesce(length(output_json),0)+coalesce(length(error_json),0)
      +coalesce(length(CAST(trigger_cron AS BLOB))+16,0)
      +length(CAST(definition_name AS BLOB))+length(CAST(external_instance_id AS BLOB))+length(CAST(class_name AS BLOB))
      +(SELECT history_bytes FROM workflow_accounting WHERE id=OLD.instance_id)
    WHERE id=OLD.instance_id AND capability_version=1;
END;

CREATE TRIGGER workflow_events_insert_accounting AFTER INSERT ON workflow_events
WHEN (SELECT capability_version FROM workflow_instances WHERE id=NEW.instance_id)=1
BEGIN
  UPDATE workflow_instances SET
    registered_step_count=(SELECT registered FROM workflow_accounting WHERE id=NEW.instance_id),
    settled_step_count=(SELECT settled FROM workflow_accounting WHERE id=NEW.instance_id),
    completed_step_count=(SELECT completed FROM workflow_accounting WHERE id=NEW.instance_id),
    event_count=(SELECT event_count FROM workflow_accounting WHERE id=NEW.instance_id),
    event_bytes=(SELECT event_bytes FROM workflow_accounting WHERE id=NEW.instance_id),
    next_wake_at_ms=(SELECT next_wake FROM workflow_accounting WHERE id=NEW.instance_id),
    state_bytes=256+length(input_json)+coalesce(length(output_json),0)+coalesce(length(error_json),0)
      +coalesce(length(CAST(trigger_cron AS BLOB))+16,0)
      +length(CAST(definition_name AS BLOB))+length(CAST(external_instance_id AS BLOB))+length(CAST(class_name AS BLOB))
      +(SELECT history_bytes FROM workflow_accounting WHERE id=NEW.instance_id),
    next_event_seq=next_event_seq+1
    WHERE id=NEW.instance_id AND capability_version=1;
END;

CREATE TRIGGER workflow_generation_guard BEFORE UPDATE OF instance_generation,last_restart_operation_id,
  last_restart_from_name,last_restart_from_count,last_restart_from_kind,last_restart_target_ordinal ON workflow_instances
WHEN OLD.capability_version=1 AND NOT EXISTS(SELECT 1 FROM workflow_mutation_context c WHERE c.instance_id=OLD.id
  AND c.kind='restart' AND c.creation_nonce=OLD.creation_nonce AND c.expected_generation=OLD.instance_generation
  AND c.target_generation=NEW.instance_generation AND NEW.last_restart_operation_id=c.operation_id
  AND NEW.last_restart_from_name IS c.restart_from_name AND NEW.last_restart_from_count IS c.restart_from_count
  AND NEW.last_restart_from_kind IS c.restart_from_kind AND NEW.last_restart_target_ordinal IS c.restart_target_ordinal
  AND NEW.state='queued' AND NEW.registered_step_count=0 AND NEW.event_count=0
  AND NEW.next_event_seq=c.restart_next_event_seq AND NEW.has_activated=(c.restart_retain_step_count>0))
BEGIN SELECT RAISE(ABORT,'workflow generation requires exact restart'); END;

CREATE TRIGGER workflow_instance_accounting_guard BEFORE UPDATE ON workflow_instances
WHEN OLD.capability_version=1 AND EXISTS(SELECT 1 FROM workflow_accounting a WHERE a.id=OLD.id AND (
  NEW.registered_step_count!=a.registered OR NEW.settled_step_count!=a.settled OR NEW.completed_step_count!=a.completed
  OR NEW.event_count!=a.event_count OR NEW.event_bytes!=a.event_bytes OR NEW.next_wake_at_ms IS NOT a.next_wake
  OR NEW.state_bytes!=256+length(NEW.input_json)+coalesce(length(NEW.output_json),0)+coalesce(length(NEW.error_json),0)
    +coalesce(length(CAST(NEW.trigger_cron AS BLOB))+16,0)
    +length(CAST(NEW.definition_name AS BLOB))+length(CAST(NEW.external_instance_id AS BLOB))+length(CAST(NEW.class_name AS BLOB))+a.history_bytes))
BEGIN SELECT RAISE(ABORT,'workflow durable accounting'); END;

CREATE TRIGGER workflow_instance_delete_guard BEFORE DELETE ON workflow_instances
WHEN OLD.capability_version=1 AND NOT EXISTS(SELECT 1 FROM workflow_mutation_context c WHERE c.instance_id=OLD.id
  AND c.kind='purge' AND c.creation_nonce=OLD.creation_nonce AND c.expected_generation=OLD.instance_generation
  AND OLD.state IN ('complete','errored','terminated') AND OLD.run_token IS NULL)
BEGIN SELECT RAISE(ABORT,'workflow deletion requires exact purge'); END;

CREATE TRIGGER workflow_instance_frontier_guard BEFORE UPDATE OF state ON workflow_instances
WHEN OLD.capability_version=1 AND NOT EXISTS(SELECT 1 FROM workflow_mutation_context WHERE instance_id=OLD.id) AND (
  (NEW.state!='running' AND EXISTS(SELECT 1 FROM workflow_steps WHERE instance_id=OLD.id AND state='running')) OR
  (NEW.state='complete' AND NEW.settled_step_count!=NEW.registered_step_count) OR
  (NEW.state IN ('complete','errored','terminated') AND EXISTS(SELECT 1 FROM workflow_steps WHERE instance_id=OLD.id AND state IN ('pending','delay_pending','waiting','retry_wait'))) OR
  (NEW.state='waiting' AND (NEW.next_wake_at_ms IS NULL OR EXISTS(SELECT 1 FROM workflow_steps WHERE instance_id=OLD.id AND state IN ('pending','delay_pending'))))
) BEGIN SELECT RAISE(ABORT,'workflow durable unsettled frontier'); END;

CREATE TRIGGER workflow_instance_identity_guard BEFORE UPDATE OF id,definition_id,definition_name,
  external_instance_id,workflow_version_id,worker_id,worker_version_id,worker_code_sha256,class_name,creation_nonce,creation_operation_id,creation_batch_id,
  loader_schema_version,capability_version,descriptor_sha256,input_json,created_at_ms,success_retention_ms,error_retention_ms
ON workflow_instances WHEN OLD.capability_version=1
BEGIN SELECT RAISE(ABORT,'workflow durable identity is immutable'); END;

CREATE TRIGGER workflow_instance_insert_guard BEFORE INSERT ON workflow_instances
WHEN NEW.capability_version=1 AND (NEW.state!='queued' OR NEW.instance_generation!=1 OR NEW.completed_step_count!=0
  OR NEW.registered_step_count!=0 OR NEW.settled_step_count!=0 OR NEW.event_count!=0 OR NEW.event_bytes!=0
  OR NEW.next_event_seq!=1 OR NEW.has_activated!=0 OR NEW.rollback_requested!=0 OR NEW.last_restart_operation_id IS NOT NULL
  OR NEW.last_restart_from_name IS NOT NULL OR NEW.last_restart_from_count IS NOT NULL
  OR NEW.last_restart_from_kind IS NOT NULL OR NEW.last_restart_target_ordinal IS NOT NULL
  OR NEW.next_wake_at_ms IS NOT NULL OR NEW.state_bytes!=256+length(NEW.input_json)
    +coalesce(length(CAST(NEW.trigger_cron AS BLOB))+16,0)
    +length(CAST(NEW.definition_name AS BLOB))+length(CAST(NEW.external_instance_id AS BLOB))+length(CAST(NEW.class_name AS BLOB)))
BEGIN SELECT RAISE(ABORT,'workflow durable initial state'); END;

CREATE TRIGGER workflow_instance_run_guard BEFORE UPDATE ON workflow_instances
WHEN OLD.capability_version=1 AND NOT EXISTS(SELECT 1 FROM workflow_mutation_context WHERE instance_id=OLD.id) AND (
  (OLD.state='running' AND NEW.state='running' AND (NEW.run_token!=OLD.run_token
    OR NEW.run_claimed_at_ms!=OLD.run_claimed_at_ms OR NEW.run_lease_until_ms<OLD.run_lease_until_ms
    OR NEW.pause_requested<OLD.pause_requested OR NEW.yield_requested<OLD.yield_requested)) OR
  (OLD.state='running' AND NEW.state IN ('queued','waiting','paused') AND NEW.updated_at_ms<OLD.run_lease_until_ms
    AND OLD.yield_requested=0 AND OLD.pause_requested=0 AND NEW.rollback_requested=0) OR
  (OLD.state='running' AND NEW.state IN ('complete','errored','terminated') AND NEW.updated_at_ms>=OLD.run_lease_until_ms) OR
  (NEW.state='running' AND NEW.has_activated!=1)
) BEGIN SELECT RAISE(ABORT,'workflow durable run fence'); END;

CREATE TRIGGER workflow_instance_terminal_guard BEFORE UPDATE ON workflow_instances
WHEN OLD.capability_version=1 AND OLD.state IN ('complete','errored','terminated') AND NOT EXISTS(
  SELECT 1 FROM workflow_mutation_context c WHERE c.instance_id=OLD.id AND c.creation_nonce=OLD.creation_nonce
    AND c.expected_generation=OLD.instance_generation AND c.kind IN ('restart','purge'))
BEGIN SELECT RAISE(ABORT,'workflow terminal history is immutable'); END;

CREATE TRIGGER workflow_instance_transition_guard BEFORE UPDATE OF state ON workflow_instances
WHEN OLD.capability_version=1 AND NEW.state!=OLD.state AND NOT (
  (OLD.state='queued' AND NEW.state IN ('running','paused','terminated')) OR
  (OLD.state='running' AND NEW.state IN ('queued','waiting','paused','complete','errored','terminated')) OR
  (OLD.state='waiting' AND NEW.state IN ('queued','paused','terminated')) OR
  (OLD.state='paused' AND NEW.state IN ('queued','waiting','terminated')) OR
  (NEW.state='queued' AND EXISTS(SELECT 1 FROM workflow_mutation_context c WHERE c.instance_id=OLD.id
    AND c.kind='restart' AND c.creation_nonce=OLD.creation_nonce AND c.expected_generation=OLD.instance_generation
    AND c.target_generation=NEW.instance_generation AND c.operation_id=NEW.last_restart_operation_id))
) BEGIN SELECT RAISE(ABORT,'workflow durable state transition'); END;

CREATE TRIGGER workflow_progress_insert_guard BEFORE INSERT ON workflow_operation_progress
WHEN NOT (
  (NEW.outcome='rejected' AND EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=NEW.instance_id
    AND i.capability_version=1 AND i.creation_nonce=NEW.creation_nonce AND i.instance_generation=NEW.expected_generation)) OR
  (NEW.outcome='applied' AND NEW.kind='restart' AND EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=NEW.instance_id
    AND i.creation_nonce=NEW.creation_nonce AND i.instance_generation=NEW.target_generation
    AND i.last_restart_operation_id=NEW.operation_id AND i.last_restart_from_name IS NEW.restart_from_name
    AND i.last_restart_from_count IS NEW.restart_from_count AND i.last_restart_from_kind IS NEW.restart_from_kind
    AND i.last_restart_target_ordinal IS NEW.restart_target_ordinal)) OR
  (NEW.outcome='applied' AND NEW.kind='purge' AND EXISTS(SELECT 1 FROM workflow_gc_receipts r WHERE r.instance_id=NEW.instance_id
    AND r.creation_nonce=NEW.creation_nonce AND r.instance_generation=NEW.expected_generation AND r.operation_id=NEW.operation_id))
) BEGIN SELECT RAISE(ABORT,'workflow operation result lacks exact authority'); END;

CREATE TRIGGER workflow_progress_update_guard BEFORE UPDATE ON workflow_operation_progress
WHEN NEW.instance_id!=OLD.instance_id OR NEW.creation_nonce!=OLD.creation_nonce OR NEW.operation_sequence<=OLD.operation_sequence OR NOT (
  (NEW.outcome='rejected' AND EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=NEW.instance_id
    AND i.capability_version=1 AND i.creation_nonce=NEW.creation_nonce AND i.instance_generation=NEW.expected_generation)) OR
  (NEW.outcome='applied' AND NEW.kind='restart' AND EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=NEW.instance_id
    AND i.creation_nonce=NEW.creation_nonce AND i.instance_generation=NEW.target_generation
    AND i.last_restart_operation_id=NEW.operation_id AND i.last_restart_from_name IS NEW.restart_from_name
    AND i.last_restart_from_count IS NEW.restart_from_count AND i.last_restart_from_kind IS NEW.restart_from_kind
    AND i.last_restart_target_ordinal IS NEW.restart_target_ordinal)) OR
  (NEW.outcome='applied' AND NEW.kind='purge' AND EXISTS(SELECT 1 FROM workflow_gc_receipts r WHERE r.instance_id=NEW.instance_id
    AND r.creation_nonce=NEW.creation_nonce AND r.instance_generation=NEW.expected_generation AND r.operation_id=NEW.operation_id))
) BEGIN SELECT RAISE(ABORT,'workflow operation result is not a newer exact decision'); END;

CREATE TRIGGER workflow_receipt_insert_guard BEFORE INSERT ON workflow_gc_receipts
WHEN EXISTS(SELECT 1 FROM workflow_instances WHERE id=NEW.instance_id) OR NOT EXISTS(
  SELECT 1 FROM workflow_mutation_context c WHERE c.kind='purge' AND c.operation_id=NEW.operation_id
    AND c.instance_id=NEW.instance_id AND c.creation_nonce=NEW.creation_nonce AND c.expected_generation=NEW.instance_generation
    AND c.authorized_at_ms=NEW.deleted_at_ms)
BEGIN SELECT RAISE(ABORT,'workflow purge receipt requires exact deletion'); END;

CREATE TRIGGER workflow_step_delete_guard BEFORE DELETE ON workflow_steps
WHEN OLD.config_sha256 IS NOT NULL AND NOT EXISTS(SELECT 1 FROM workflow_mutation_context c JOIN workflow_instances i ON i.id=c.instance_id
  WHERE c.instance_id=OLD.instance_id AND c.expected_generation=OLD.instance_generation AND c.kind IN ('restart','purge')
    AND c.creation_nonce=i.creation_nonce AND c.expected_generation=i.instance_generation)
BEGIN SELECT RAISE(ABORT,'workflow step history requires exact operation'); END;

CREATE TRIGGER workflow_step_dependencies_delete_accounting AFTER DELETE ON workflow_step_dependencies
WHEN (SELECT capability_version FROM workflow_instances WHERE id=OLD.instance_id)=1
BEGIN
  UPDATE workflow_instances SET
    registered_step_count=(SELECT registered FROM workflow_accounting WHERE id=OLD.instance_id),
    settled_step_count=(SELECT settled FROM workflow_accounting WHERE id=OLD.instance_id),
    completed_step_count=(SELECT completed FROM workflow_accounting WHERE id=OLD.instance_id),
    event_count=(SELECT event_count FROM workflow_accounting WHERE id=OLD.instance_id),
    event_bytes=(SELECT event_bytes FROM workflow_accounting WHERE id=OLD.instance_id),
    next_wake_at_ms=(SELECT next_wake FROM workflow_accounting WHERE id=OLD.instance_id),
    state_bytes=256+length(input_json)+coalesce(length(output_json),0)+coalesce(length(error_json),0)
      +coalesce(length(CAST(trigger_cron AS BLOB))+16,0)
      +length(CAST(definition_name AS BLOB))+length(CAST(external_instance_id AS BLOB))+length(CAST(class_name AS BLOB))
      +(SELECT history_bytes FROM workflow_accounting WHERE id=OLD.instance_id)
    WHERE id=OLD.instance_id AND capability_version=1;
END;

CREATE TRIGGER workflow_step_dependencies_insert_accounting AFTER INSERT ON workflow_step_dependencies
WHEN (SELECT capability_version FROM workflow_instances WHERE id=NEW.instance_id)=1
BEGIN
  UPDATE workflow_instances SET
    registered_step_count=(SELECT registered FROM workflow_accounting WHERE id=NEW.instance_id),
    settled_step_count=(SELECT settled FROM workflow_accounting WHERE id=NEW.instance_id),
    completed_step_count=(SELECT completed FROM workflow_accounting WHERE id=NEW.instance_id),
    event_count=(SELECT event_count FROM workflow_accounting WHERE id=NEW.instance_id),
    event_bytes=(SELECT event_bytes FROM workflow_accounting WHERE id=NEW.instance_id),
    next_wake_at_ms=(SELECT next_wake FROM workflow_accounting WHERE id=NEW.instance_id),
    state_bytes=256+length(input_json)+coalesce(length(output_json),0)+coalesce(length(error_json),0)
      +coalesce(length(CAST(trigger_cron AS BLOB))+16,0)
      +length(CAST(definition_name AS BLOB))+length(CAST(external_instance_id AS BLOB))+length(CAST(class_name AS BLOB))
      +(SELECT history_bytes FROM workflow_accounting WHERE id=NEW.instance_id)
    WHERE id=NEW.instance_id AND capability_version=1;
END;

CREATE TRIGGER workflow_step_insert_guard BEFORE INSERT ON workflow_steps
WHEN NEW.config_sha256 IS NOT NULL
BEGIN
  SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM workflow_mutation_context c WHERE c.instance_id=NEW.instance_id
      AND c.kind='restart' AND c.target_generation=NEW.instance_generation
      AND NEW.ordinal<c.restart_retain_step_count
      AND ((NEW.ordinal<c.restart_target_ordinal AND NEW.state='complete'
          AND NEW.run_token IS NULL AND NEW.step_token IS NULL AND NEW.error_json IS NULL AND NEW.error_code IS NULL)
        OR (NEW.ordinal>=c.restart_target_ordinal AND NEW.attempt=0 AND NEW.output_json IS NULL
          AND NEW.error_json IS NULL AND NEW.error_code IS NULL AND NEW.completed_at_ms IS NULL
          AND NEW.cancelled_at_ms IS NULL AND NEW.run_token IS NULL AND NEW.step_token IS NULL
          AND NEW.attempt_started_at_ms IS NULL AND NEW.attempt_deadline_at_ms IS NULL
          AND NEW.retry_delay_ms IS NULL
          AND ((NEW.kind='do' AND NEW.state='pending' AND NEW.due_at_ms IS NULL AND NEW.event_buffer_ceiling IS NULL)
            OR (NEW.kind IN ('sleep','sleep_until','wait_event') AND NEW.state='waiting' AND NEW.due_at_ms IS NOT NULL)))))
    AND NOT EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=NEW.instance_id
    AND i.capability_version=1 AND i.instance_generation=NEW.instance_generation AND i.state='running'
    AND i.run_lease_until_ms>NEW.updated_at_ms AND i.pause_requested=0 AND i.yield_requested=0
    AND i.rollback_requested=json_extract(CAST(NEW.config_json AS TEXT),'$.rollbackStep')
    AND i.registered_step_count=NEW.ordinal)
    OR (EXISTS(SELECT 1 FROM workflow_mutation_context c WHERE c.instance_id=NEW.instance_id
      AND c.kind='restart') AND json_extract(CAST(NEW.config_json AS TEXT),'$.rollbackStep')!=0)
    OR (NOT EXISTS(SELECT 1 FROM workflow_mutation_context c WHERE c.instance_id=NEW.instance_id
        AND c.kind='restart' AND c.target_generation=NEW.instance_generation) AND
      (NEW.attempt!=0 OR NEW.state!=CASE WHEN NEW.kind='do' THEN 'pending' ELSE 'waiting' END
      OR NEW.name_count!=1+(SELECT COUNT(*) FROM workflow_steps s WHERE s.instance_id=NEW.instance_id AND s.kind=NEW.kind AND s.name=NEW.name)
      OR (NEW.ordinal!=NEW.batch_first_ordinal AND NOT EXISTS(SELECT 1 FROM workflow_steps s WHERE s.instance_id=NEW.instance_id
        AND s.ordinal=NEW.batch_first_ordinal AND s.batch_size=NEW.batch_size AND s.dependency_count=NEW.dependency_count))))
    THEN RAISE(ABORT,'workflow durable registration frontier') END;
  SELECT CASE WHEN (NOT EXISTS(SELECT 1 FROM workflow_mutation_context WHERE instance_id=NEW.instance_id AND kind='restart')
      OR NEW.ordinal>=(SELECT restart_target_ordinal FROM workflow_mutation_context
        WHERE instance_id=NEW.instance_id AND kind='restart')) AND
    ((NEW.kind='wait_event' AND NEW.event_buffer_ceiling!=(SELECT next_event_seq-1 FROM workflow_instances WHERE id=NEW.instance_id))
    OR (NEW.kind='sleep' AND NEW.due_at_ms IS NOT NEW.started_at_ms+json_extract(CAST(NEW.config_json AS TEXT),'$.durationMs'))
    OR (NEW.kind='sleep_until' AND NEW.due_at_ms IS NOT json_extract(CAST(NEW.config_json AS TEXT),'$.timestampMs'))
    OR (NEW.kind='wait_event' AND NEW.due_at_ms IS NOT NEW.started_at_ms+json_extract(CAST(NEW.config_json AS TEXT),'$.timeoutMs')))
    THEN RAISE(ABORT,'workflow durable registration deadline') END;
END;

CREATE TRIGGER workflow_step_transition_guard BEFORE UPDATE ON workflow_steps
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
  (OLD.state='running' AND NEW.state='delay_pending' AND OLD.attempt_deadline_at_ms<=NEW.updated_at_ms
    AND NEW.error_code='WORKFLOW_STEP_TIMEOUT' AND EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=OLD.instance_id
      AND i.state='running' AND i.run_token=OLD.run_token)) OR
  (OLD.state='pending' AND OLD.attempt>0 AND OLD.attempt_deadline_at_ms<=NEW.updated_at_ms
    AND NEW.state IN ('failed','retry_wait') AND EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=OLD.instance_id
      AND i.state IN ('queued','running','waiting','paused'))) OR
  (OLD.state='pending' AND OLD.attempt>0 AND OLD.attempt_deadline_at_ms<=NEW.updated_at_ms
    AND NEW.state='delay_pending' AND NEW.error_code='WORKFLOW_STEP_TIMEOUT' AND EXISTS(SELECT 1 FROM workflow_instances i
      WHERE i.id=OLD.instance_id AND i.state IN ('queued','running','waiting','paused'))) OR
  (OLD.state='delay_pending' AND NEW.state IN ('failed','retry_wait') AND EXISTS(SELECT 1 FROM workflow_instances i
    WHERE i.id=OLD.instance_id AND i.state IN ('queued','running','waiting','paused'))) OR
  (OLD.state='waiting' AND NEW.state IN ('complete','failed') AND EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=OLD.instance_id
    AND i.state IN ('queued','running','waiting','paused'))) OR
  (NEW.state='cancelled' AND OLD.state IN ('pending','running','delay_pending','waiting','retry_wait') AND EXISTS(
    SELECT 1 FROM workflow_instances i WHERE i.id=OLD.instance_id AND i.state IN ('queued','running','waiting','paused')))
) BEGIN SELECT RAISE(ABORT,'workflow durable step fence'); END;

CREATE TRIGGER workflow_steps_delete_accounting AFTER DELETE ON workflow_steps
WHEN (SELECT capability_version FROM workflow_instances WHERE id=OLD.instance_id)=1
BEGIN
  UPDATE workflow_instances SET
    registered_step_count=(SELECT registered FROM workflow_accounting WHERE id=OLD.instance_id),
    settled_step_count=(SELECT settled FROM workflow_accounting WHERE id=OLD.instance_id),
    completed_step_count=(SELECT completed FROM workflow_accounting WHERE id=OLD.instance_id),
    event_count=(SELECT event_count FROM workflow_accounting WHERE id=OLD.instance_id),
    event_bytes=(SELECT event_bytes FROM workflow_accounting WHERE id=OLD.instance_id),
    next_wake_at_ms=(SELECT next_wake FROM workflow_accounting WHERE id=OLD.instance_id),
    state_bytes=256+length(input_json)+coalesce(length(output_json),0)+coalesce(length(error_json),0)
      +coalesce(length(CAST(trigger_cron AS BLOB))+16,0)
      +length(CAST(definition_name AS BLOB))+length(CAST(external_instance_id AS BLOB))+length(CAST(class_name AS BLOB))
      +(SELECT history_bytes FROM workflow_accounting WHERE id=OLD.instance_id)
    WHERE id=OLD.instance_id AND capability_version=1;
END;

CREATE TRIGGER workflow_steps_insert_accounting AFTER INSERT ON workflow_steps
WHEN (SELECT capability_version FROM workflow_instances WHERE id=NEW.instance_id)=1
BEGIN
  UPDATE workflow_instances SET
    registered_step_count=(SELECT registered FROM workflow_accounting WHERE id=NEW.instance_id),
    settled_step_count=(SELECT settled FROM workflow_accounting WHERE id=NEW.instance_id),
    completed_step_count=(SELECT completed FROM workflow_accounting WHERE id=NEW.instance_id),
    event_count=(SELECT event_count FROM workflow_accounting WHERE id=NEW.instance_id),
    event_bytes=(SELECT event_bytes FROM workflow_accounting WHERE id=NEW.instance_id),
    next_wake_at_ms=(SELECT next_wake FROM workflow_accounting WHERE id=NEW.instance_id),
    state_bytes=256+length(input_json)+coalesce(length(output_json),0)+coalesce(length(error_json),0)
      +coalesce(length(CAST(trigger_cron AS BLOB))+16,0)
      +length(CAST(definition_name AS BLOB))+length(CAST(external_instance_id AS BLOB))+length(CAST(class_name AS BLOB))
      +(SELECT history_bytes FROM workflow_accounting WHERE id=NEW.instance_id)
    WHERE id=NEW.instance_id AND capability_version=1;
END;

CREATE TRIGGER workflow_steps_update_accounting AFTER UPDATE ON workflow_steps
WHEN (SELECT capability_version FROM workflow_instances WHERE id=NEW.instance_id)=1
BEGIN
  UPDATE workflow_instances SET
    registered_step_count=(SELECT registered FROM workflow_accounting WHERE id=NEW.instance_id),
    settled_step_count=(SELECT settled FROM workflow_accounting WHERE id=NEW.instance_id),
    completed_step_count=(SELECT completed FROM workflow_accounting WHERE id=NEW.instance_id),
    event_count=(SELECT event_count FROM workflow_accounting WHERE id=NEW.instance_id),
    event_bytes=(SELECT event_bytes FROM workflow_accounting WHERE id=NEW.instance_id),
    next_wake_at_ms=(SELECT next_wake FROM workflow_accounting WHERE id=NEW.instance_id),
    state_bytes=256+length(input_json)+coalesce(length(output_json),0)+coalesce(length(error_json),0)
      +coalesce(length(CAST(trigger_cron AS BLOB))+16,0)
      +length(CAST(definition_name AS BLOB))+length(CAST(external_instance_id AS BLOB))+length(CAST(class_name AS BLOB))
      +(SELECT history_bytes FROM workflow_accounting WHERE id=NEW.instance_id)
    WHERE id=NEW.instance_id AND capability_version=1;
END;

-- Replacing the parent leaves deferred DROP counters; verify every foreign key before clearing them.
CREATE TEMP TABLE workflow_retention_foreign_keys_valid (
  violations INTEGER NOT NULL CHECK(violations=0)
);
INSERT INTO workflow_retention_foreign_keys_valid SELECT count(*) FROM pragma_foreign_key_check;
DROP TABLE workflow_retention_foreign_keys_valid;
PRAGMA defer_foreign_keys=OFF;
