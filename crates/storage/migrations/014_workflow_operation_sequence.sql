-- A bounded per-instance operation sequence fences delayed work after a rejected saga.
ALTER TABLE workflow_instance_referrers ADD COLUMN operation_sequence INTEGER NOT NULL DEFAULT 0 CHECK(operation_sequence>=0);
ALTER TABLE workflow_instance_operations ADD COLUMN operation_sequence INTEGER NOT NULL DEFAULT 1 CHECK(operation_sequence>=1);
UPDATE workflow_instance_referrers SET operation_sequence=1 WHERE instance_generation>1
  OR instance_id IN (SELECT instance_id FROM workflow_instance_operations);

CREATE TRIGGER workflow_operation_sequence_reservation_guard BEFORE UPDATE OF operation_sequence ON workflow_instance_referrers
WHEN NEW.operation_sequence!=OLD.operation_sequence+1 OR OLD.operation_sequence=9223372036854775807
  OR OLD.state NOT IN ('live','retained') OR EXISTS(SELECT 1 FROM workflow_instance_operations WHERE instance_id=OLD.instance_id)
BEGIN SELECT RAISE(ABORT,'workflow operation sequence requires a free intent slot'); END;
CREATE TRIGGER workflow_operation_sequence_insert_guard BEFORE INSERT ON workflow_instance_operations
WHEN NOT EXISTS(SELECT 1 FROM workflow_instance_referrers r WHERE r.instance_id=NEW.instance_id
  AND r.operation_sequence=NEW.operation_sequence AND r.operation_sequence>=1)
BEGIN SELECT RAISE(ABORT,'workflow operation sequence does not match its reservation'); END;
CREATE TRIGGER workflow_operation_sequence_immutable BEFORE UPDATE OF operation_sequence ON workflow_instance_operations
BEGIN SELECT RAISE(ABORT,'workflow operation sequence is immutable'); END;
