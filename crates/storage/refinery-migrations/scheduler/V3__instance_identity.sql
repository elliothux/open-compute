-- A scheduler database belongs to exactly one control authority. Historical
-- projections carry UUID-form instance identities; reject mixed ownership
-- before the per-row columns are removed from the current schema.
CREATE TABLE scheduler_identity (
  singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
  instance_id TEXT NOT NULL
    CHECK(length(instance_id) = 32 AND instance_id = lower(instance_id)
      AND instance_id NOT GLOB '*[^0-9a-f]*')
) STRICT;

WITH owners AS (
  SELECT account_id FROM queue_state
  UNION SELECT account_id FROM cron_schedules
  UNION SELECT account_id FROM workflow_instances
)
INSERT INTO scheduler_identity(singleton, instance_id)
SELECT 1, replace(account_id, '-', '') FROM owners;

DROP TRIGGER cron_schedules_identity_guard;
DROP TRIGGER workflow_instance_identity_guard;
DROP INDEX workflow_instances_account;
DROP INDEX workflow_instances_fair;

ALTER TABLE queue_state DROP COLUMN account_id;
ALTER TABLE cron_schedules DROP COLUMN account_id;
ALTER TABLE workflow_instances DROP COLUMN account_id;

CREATE TRIGGER cron_schedules_identity_guard
BEFORE UPDATE OF activation_id, worker_id, version_id, execution_generation,
  activation_generation, expression, expression_sha256, parser_version ON cron_schedules
BEGIN
  SELECT RAISE(ABORT, 'cron schedule identity is immutable');
END;

CREATE TRIGGER workflow_instance_identity_guard BEFORE UPDATE OF id,definition_id,definition_name,
  external_instance_id,workflow_version_id,worker_id,worker_version_id,worker_code_sha256,class_name,creation_nonce,creation_operation_id,creation_batch_id,
  loader_schema_version,capability_version,descriptor_sha256,input_json,created_at_ms,success_retention_ms,error_retention_ms
ON workflow_instances WHEN OLD.capability_version=1
BEGIN SELECT RAISE(ABORT,'workflow durable identity is immutable'); END;

CREATE INDEX workflow_instances_definition ON workflow_instances(definition_id,state);
CREATE INDEX workflow_instances_fair ON workflow_instances(has_activated,next_run_at_ms,created_at_ms,id)
  WHERE state='queued';
