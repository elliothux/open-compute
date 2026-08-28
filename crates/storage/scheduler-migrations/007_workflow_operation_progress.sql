-- One durable result per internal UUID, including definitive rejection. A higher control
-- sequence may replace it; stale retries can never reapply an earlier operation.
CREATE TABLE workflow_operation_progress (
  instance_id TEXT PRIMARY KEY,
  operation_id TEXT NOT NULL UNIQUE,
  operation_sequence INTEGER NOT NULL CHECK(operation_sequence>=1),
  creation_nonce BLOB NOT NULL CHECK(length(creation_nonce)=32),
  expected_generation INTEGER NOT NULL CHECK(expected_generation>=1),
  target_generation INTEGER NOT NULL CHECK(target_generation>=1),
  kind TEXT NOT NULL CHECK(kind IN ('restart','purge')),
  outcome TEXT NOT NULL CHECK(outcome IN ('applied','rejected')),
  error_code TEXT,
  decided_at_ms INTEGER NOT NULL,
  CHECK((kind='restart' AND expected_generation<9223372036854775807 AND target_generation=expected_generation+1)
    OR (kind='purge' AND target_generation=expected_generation)),
  CHECK((outcome='applied' AND error_code IS NULL) OR
    (outcome='rejected' AND error_code IN ('WORKFLOW_INSTANCE_NOT_FOUND','WORKFLOW_INSTANCE_STATE_CONFLICT','WORKFLOW_STATE_QUOTA_EXCEEDED')))
) STRICT;

-- Preserve any pre-sequence committed restart or purge proof. Both matching control
-- reservations and unfinished old intents are initialized to sequence one by migration 014.
INSERT INTO workflow_operation_progress SELECT id,last_restart_operation_id,1,creation_nonce,instance_generation-1,
  instance_generation,'restart','applied',NULL,updated_at_ms FROM workflow_instances WHERE last_restart_operation_id IS NOT NULL;
INSERT INTO workflow_operation_progress SELECT instance_id,operation_id,1,creation_nonce,instance_generation,
  instance_generation,'purge','applied',NULL,deleted_at_ms FROM workflow_gc_receipts;

CREATE TRIGGER workflow_progress_insert_guard BEFORE INSERT ON workflow_operation_progress
WHEN NOT (
  (NEW.outcome='rejected' AND EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=NEW.instance_id
    AND i.capability_version=2 AND i.creation_nonce=NEW.creation_nonce AND i.instance_generation=NEW.expected_generation)) OR
  (NEW.outcome='applied' AND NEW.kind='restart' AND EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=NEW.instance_id
    AND i.creation_nonce=NEW.creation_nonce AND i.instance_generation=NEW.target_generation AND i.last_restart_operation_id=NEW.operation_id)) OR
  (NEW.outcome='applied' AND NEW.kind='purge' AND EXISTS(SELECT 1 FROM workflow_gc_receipts r WHERE r.instance_id=NEW.instance_id
    AND r.creation_nonce=NEW.creation_nonce AND r.instance_generation=NEW.expected_generation AND r.operation_id=NEW.operation_id))
) BEGIN SELECT RAISE(ABORT,'workflow operation result lacks exact authority'); END;
CREATE TRIGGER workflow_progress_update_guard BEFORE UPDATE ON workflow_operation_progress
WHEN NEW.instance_id!=OLD.instance_id OR NEW.creation_nonce!=OLD.creation_nonce OR NEW.operation_sequence<=OLD.operation_sequence OR NOT (
  (NEW.outcome='rejected' AND EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=NEW.instance_id
    AND i.capability_version=2 AND i.creation_nonce=NEW.creation_nonce AND i.instance_generation=NEW.expected_generation)) OR
  (NEW.outcome='applied' AND NEW.kind='restart' AND EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=NEW.instance_id
    AND i.creation_nonce=NEW.creation_nonce AND i.instance_generation=NEW.target_generation AND i.last_restart_operation_id=NEW.operation_id)) OR
  (NEW.outcome='applied' AND NEW.kind='purge' AND EXISTS(SELECT 1 FROM workflow_gc_receipts r WHERE r.instance_id=NEW.instance_id
    AND r.creation_nonce=NEW.creation_nonce AND r.instance_generation=NEW.expected_generation AND r.operation_id=NEW.operation_id))
) BEGIN SELECT RAISE(ABORT,'workflow operation result is not a newer exact decision'); END;
CREATE TRIGGER workflow_progress_delete_guard BEFORE DELETE ON workflow_operation_progress
WHEN OLD.outcome!='applied' OR OLD.kind!='purge' OR NOT EXISTS(SELECT 1 FROM workflow_mutation_context c
  WHERE c.kind='acknowledge_purge' AND c.instance_id=OLD.instance_id AND c.operation_id=OLD.operation_id
    AND c.creation_nonce=OLD.creation_nonce AND c.expected_generation=OLD.expected_generation)
BEGIN SELECT RAISE(ABORT,'workflow operation watermark requires acknowledged purge'); END;
CREATE TRIGGER workflow_progress_acknowledge_guard BEFORE DELETE ON workflow_mutation_context
WHEN OLD.kind='acknowledge_purge' AND EXISTS(SELECT 1 FROM workflow_operation_progress WHERE instance_id=OLD.instance_id)
BEGIN SELECT RAISE(ABORT,'workflow purge watermark is not swept'); END;
