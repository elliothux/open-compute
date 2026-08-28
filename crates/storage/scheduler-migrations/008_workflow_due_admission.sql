-- Keep ready admission and due maintenance on live partial indexes, not retained history.
CREATE INDEX workflow_instances_fair ON workflow_instances(has_activated,account_id,next_run_at_ms,created_at_ms,id)
  WHERE state='queued';
CREATE INDEX workflow_steps_wait_due ON workflow_steps(due_at_ms,instance_id,ordinal)
  WHERE state='waiting';
CREATE INDEX workflow_steps_retry_due ON workflow_steps(due_at_ms,instance_id,ordinal)
  WHERE state='retry_wait';
CREATE INDEX workflow_steps_pending_timeout ON workflow_steps(attempt_deadline_at_ms,instance_id,ordinal)
  WHERE state='pending' AND attempt>0;

-- SQLite CHECK accepts NULL; a rejected durable operation must name its decision.
CREATE TRIGGER workflow_progress_rejection_insert_guard BEFORE INSERT ON workflow_operation_progress
WHEN NEW.outcome='rejected' AND NEW.error_code IS NULL
BEGIN SELECT RAISE(ABORT,'workflow rejection requires a stable error code'); END;
CREATE TRIGGER workflow_progress_rejection_update_guard BEFORE UPDATE ON workflow_operation_progress
WHEN NEW.outcome='rejected' AND NEW.error_code IS NULL
BEGIN SELECT RAISE(ABORT,'workflow rejection requires a stable error code'); END;
