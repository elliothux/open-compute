CREATE TABLE scheduled_jobs (
  id                    TEXT PRIMARY KEY,
  kind                  TEXT NOT NULL CHECK(kind = 'do_alarm'),
  namespace_resource_id TEXT NOT NULL,
  object_id             TEXT NOT NULL,
  object_generation     INTEGER NOT NULL CHECK(object_generation >= 1),
  row_token             TEXT NOT NULL,
  due_at_ms             INTEGER NOT NULL CHECK(due_at_ms > 0),
  target_version_id  TEXT NOT NULL,
  execution_generation  INTEGER NOT NULL CHECK(execution_generation >= 0),
  state                 TEXT NOT NULL CHECK(state IN (
                           'scheduled', 'claimed', 'discarding'
                         )),
  retry_count           INTEGER NOT NULL DEFAULT 0 CHECK(retry_count BETWEEN 0 AND 6),
  claim_token           TEXT,
  claim_until_ms        INTEGER,
  last_error_code       TEXT,
  created_at_ms         INTEGER NOT NULL,
  updated_at_ms         INTEGER NOT NULL,
  CHECK(length(object_id) = 64 AND object_id = lower(object_id)),
  CHECK(object_id NOT GLOB '*[^0-9a-f]*'),
  CHECK(length(row_token) BETWEEN 16 AND 128),
  CHECK((state = 'claimed') = (claim_token IS NOT NULL)),
  CHECK((state = 'claimed') = (claim_until_ms IS NOT NULL)),
  UNIQUE(namespace_resource_id, object_id, object_generation)
) STRICT;
CREATE INDEX scheduled_jobs_due
ON scheduled_jobs(due_at_ms, id)
WHERE state = 'scheduled';
CREATE INDEX scheduled_jobs_expired_claim
ON scheduled_jobs(claim_until_ms, id)
WHERE state = 'claimed';
CREATE INDEX scheduled_jobs_discarding
ON scheduled_jobs(updated_at_ms, id)
WHERE state = 'discarding';
CREATE TABLE queue_state (
  queue_id                 TEXT PRIMARY KEY
                           CHECK(length(queue_id) = 36 AND queue_id = lower(queue_id)),
  account_id               TEXT NOT NULL
                           CHECK(length(account_id) = 36 AND account_id = lower(account_id)),
  lifecycle_generation     INTEGER NOT NULL CHECK(lifecycle_generation >= 1),
  config_generation        INTEGER NOT NULL CHECK(config_generation >= 1),
  state                    TEXT NOT NULL CHECK(state IN (
                             'accepting', 'configuring', 'deleting'
                           )),
  delivery_delay_seconds   INTEGER NOT NULL
                           CHECK(delivery_delay_seconds BETWEEN 0 AND 86400),
  retention_seconds        INTEGER NOT NULL
                           CHECK(retention_seconds BETWEEN 60 AND 1209600),
  max_message_bytes        INTEGER NOT NULL CHECK(max_message_bytes > 0),
  max_batch_messages       INTEGER NOT NULL CHECK(max_batch_messages > 0),
  max_batch_bytes          INTEGER NOT NULL CHECK(max_batch_bytes > 0),
  max_backlog_bytes        INTEGER NOT NULL CHECK(max_backlog_bytes > 0),
  message_count            INTEGER NOT NULL DEFAULT 0 CHECK(message_count >= 0),
  message_bytes            INTEGER NOT NULL DEFAULT 0 CHECK(message_bytes >= 0),
  created_at_ms            INTEGER NOT NULL,
  updated_at_ms            INTEGER NOT NULL
) STRICT;
CREATE TABLE queue_messages (
  seq                  INTEGER PRIMARY KEY AUTOINCREMENT,
  id                   TEXT NOT NULL UNIQUE
                       CHECK(length(id) = 36 AND id = lower(id)),
  queue_id             TEXT NOT NULL REFERENCES queue_state(queue_id),
  queue_generation     INTEGER NOT NULL CHECK(queue_generation >= 1),
  enqueued_at_ms       INTEGER NOT NULL,
  available_at_ms      INTEGER NOT NULL,
  expires_at_ms        INTEGER NOT NULL,
  content_type         TEXT NOT NULL CHECK(content_type IN (
                         'json', 'text', 'bytes', 'v8'
                       )),
  body                 BLOB NOT NULL,
  body_bytes           INTEGER NOT NULL CHECK(body_bytes >= 0),
  state                TEXT NOT NULL DEFAULT 'ready'
                       CHECK(state IN ('ready', 'claimed')),
  attempts             INTEGER NOT NULL DEFAULT 0 CHECK(attempts >= 0),
  claim_token          BLOB,
  claim_until_ms       INTEGER,
  claimed_at_ms        INTEGER,
  claim_batch_id       TEXT,
  consumer_id          TEXT,
  consumer_generation  INTEGER,
  CHECK(body_bytes = length(body)),
  CHECK(available_at_ms >= enqueued_at_ms),
  CHECK(expires_at_ms > enqueued_at_ms),
  CHECK(
    (state = 'ready' AND claim_token IS NULL AND
      claim_until_ms IS NULL AND claimed_at_ms IS NULL)
    OR
    (state = 'claimed' AND length(claim_token) = 32 AND
      claim_until_ms IS NOT NULL AND claimed_at_ms IS NOT NULL)
  )
) STRICT;
CREATE INDEX queue_messages_due
ON queue_messages(queue_id, state, available_at_ms, seq);
CREATE INDEX queue_messages_retention
ON queue_messages(expires_at_ms, queue_id, seq);
CREATE INDEX queue_messages_oldest
ON queue_messages(queue_id, enqueued_at_ms, seq);
CREATE TRIGGER queue_messages_insert_guard
BEFORE INSERT ON queue_messages
BEGIN
  SELECT CASE WHEN NEW.state != 'ready' OR NEW.attempts != 0 OR
                   NEW.claim_token IS NOT NULL OR NEW.claim_until_ms IS NOT NULL OR
                   NEW.claimed_at_ms IS NOT NULL
    THEN RAISE(ABORT, 'queue producer may only insert ready messages') END;
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM queue_state q WHERE q.queue_id = NEW.queue_id
      AND q.state = 'accepting'
      AND q.lifecycle_generation = NEW.queue_generation
      AND NEW.body_bytes <= q.max_message_bytes
      AND NEW.expires_at_ms = NEW.enqueued_at_ms + q.retention_seconds * 1000
      AND q.message_bytes + NEW.body_bytes <= q.max_backlog_bytes
  ) THEN RAISE(ABORT, 'queue message authority invariant') END;
END;
CREATE TRIGGER queue_messages_counter_insert
AFTER INSERT ON queue_messages
BEGIN
  UPDATE queue_state
  SET message_count = message_count + 1,
      message_bytes = message_bytes + NEW.body_bytes,
      updated_at_ms = NEW.enqueued_at_ms
  WHERE queue_id = NEW.queue_id;
END;
CREATE TRIGGER queue_messages_counter_delete
AFTER DELETE ON queue_messages
BEGIN
  UPDATE queue_state
  SET message_count = message_count - 1,
      message_bytes = message_bytes - OLD.body_bytes
  WHERE queue_id = OLD.queue_id;
END;
CREATE TABLE queue_enqueue_operations (
  request_id          TEXT PRIMARY KEY
                      CHECK(length(request_id) = 36 AND request_id = lower(request_id)),
  queue_id            TEXT NOT NULL REFERENCES queue_state(queue_id),
  queue_generation    INTEGER NOT NULL CHECK(queue_generation >= 1),
  fingerprint         BLOB NOT NULL CHECK(length(fingerprint) = 32),
  response_json       TEXT NOT NULL CHECK(length(response_json) > 0),
  output_gate         INTEGER NOT NULL CHECK(output_gate IN (0, 1)),
  retention_seconds   INTEGER NOT NULL CHECK(retention_seconds BETWEEN 60 AND 1209600),
  created_at_ms       INTEGER NOT NULL,
  finalized_at_ms     INTEGER,
  expires_at_ms       INTEGER,
  CHECK(
    (output_gate = 0 AND finalized_at_ms = created_at_ms AND expires_at_ms > created_at_ms)
    OR
    (output_gate = 1 AND (
      (finalized_at_ms IS NULL AND expires_at_ms IS NULL)
      OR
      (finalized_at_ms IS NOT NULL AND expires_at_ms > finalized_at_ms)
    ))
  )
) STRICT;
CREATE INDEX queue_enqueue_operations_retention
ON queue_enqueue_operations(expires_at_ms, queue_id);
CREATE INDEX queue_enqueue_operations_queue
ON queue_enqueue_operations(queue_id, created_at_ms);
CREATE TRIGGER queue_state_delete_guard
BEFORE DELETE ON queue_state
WHEN OLD.message_count != 0 OR OLD.message_bytes != 0 OR EXISTS (
  SELECT 1 FROM queue_messages WHERE queue_id = OLD.queue_id
) OR EXISTS (
  SELECT 1 FROM queue_enqueue_operations WHERE queue_id = OLD.queue_id
)
BEGIN
  SELECT RAISE(ABORT, 'queue state has backlog');
END;
CREATE TABLE queue_consumer_state (
  consumer_id                    TEXT PRIMARY KEY
                                 CHECK(length(consumer_id) = 36 AND consumer_id = lower(consumer_id)),
  queue_id                       TEXT NOT NULL UNIQUE REFERENCES queue_state(queue_id),
  consumer_generation            INTEGER NOT NULL CHECK(consumer_generation >= 1),
  version_id                  TEXT NOT NULL
                                 CHECK(length(version_id) = 36 AND version_id = lower(version_id)),
  worker_id                      TEXT NOT NULL
                                 CHECK(length(worker_id) = 36 AND worker_id = lower(worker_id)),
  execution_generation           INTEGER NOT NULL CHECK(execution_generation >= 1),
  entrypoint                     TEXT CHECK(entrypoint IS NULL OR length(entrypoint) BETWEEN 1 AND 128),
  state                          TEXT NOT NULL CHECK(state IN (
                                   'staged', 'accepting', 'paused', 'draining', 'deleting'
                                 )),
  max_batch_size                 INTEGER NOT NULL CHECK(max_batch_size BETWEEN 1 AND 100),
  max_batch_timeout_ms           INTEGER NOT NULL CHECK(max_batch_timeout_ms BETWEEN 0 AND 60000),
  max_retries                    INTEGER NOT NULL CHECK(max_retries BETWEEN 0 AND 100),
  retry_delay_seconds            INTEGER NOT NULL CHECK(retry_delay_seconds BETWEEN 0 AND 86400),
  max_concurrency                INTEGER NOT NULL CHECK(max_concurrency BETWEEN 1 AND 4096),
  dlq_queue_id                   TEXT REFERENCES queue_state(queue_id),
  dlq_queue_generation           INTEGER,
  descriptor_sha256              BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  updated_at_ms                  INTEGER NOT NULL,
  CHECK((dlq_queue_id IS NULL) = (dlq_queue_generation IS NULL)),
  CHECK(dlq_queue_generation IS NULL OR dlq_queue_generation >= 1),
  CHECK(dlq_queue_id IS NULL OR dlq_queue_id != queue_id)
) STRICT;
CREATE TABLE queue_delivery_batches (
  id                    TEXT PRIMARY KEY
                        CHECK(length(id) = 36 AND id = lower(id)),
  queue_id              TEXT NOT NULL REFERENCES queue_state(queue_id),
  consumer_id           TEXT NOT NULL REFERENCES queue_consumer_state(consumer_id),
  consumer_generation   INTEGER NOT NULL CHECK(consumer_generation >= 1),
  version_id         TEXT NOT NULL
                        CHECK(length(version_id) = 36 AND version_id = lower(version_id)),
  execution_generation  INTEGER NOT NULL CHECK(execution_generation >= 1),
  entrypoint            TEXT CHECK(entrypoint IS NULL OR length(entrypoint) BETWEEN 1 AND 128),
  claim_token           BLOB NOT NULL CHECK(length(claim_token) = 32),
  state                 TEXT NOT NULL CHECK(state = 'claimed'),
  claimed_at_ms         INTEGER NOT NULL,
  claim_until_ms        INTEGER NOT NULL,
  message_count         INTEGER NOT NULL CHECK(message_count BETWEEN 1 AND 100),
  created_at_ms         INTEGER NOT NULL,
  CHECK(claim_until_ms > claimed_at_ms)
) STRICT;
CREATE INDEX queue_delivery_batches_expired
ON queue_delivery_batches(claim_until_ms, id);
CREATE INDEX queue_delivery_batches_consumer
ON queue_delivery_batches(consumer_id, consumer_generation, id);
CREATE INDEX queue_messages_claimed_batch
ON queue_messages(claim_batch_id, seq)
WHERE state = 'claimed';
CREATE INDEX queue_messages_batch_eligibility
ON queue_messages(queue_id, available_at_ms, seq)
WHERE state = 'ready';
CREATE TABLE queue_dlq_pending (
  message_id              TEXT PRIMARY KEY,
  source_queue_id         TEXT NOT NULL REFERENCES queue_state(queue_id),
  target_queue_id         TEXT NOT NULL REFERENCES queue_state(queue_id),
  target_queue_generation INTEGER NOT NULL CHECK(target_queue_generation >= 1),
  terminal_attempts       INTEGER NOT NULL CHECK(terminal_attempts > 0),
  next_attempt_at_ms      INTEGER NOT NULL,
  created_at_ms           INTEGER NOT NULL,
  last_error_code         TEXT CHECK(last_error_code IS NULL OR length(last_error_code) BETWEEN 1 AND 128),
  CHECK(source_queue_id != target_queue_id)
) STRICT;
CREATE INDEX queue_dlq_pending_due
ON queue_dlq_pending(next_attempt_at_ms, message_id);
CREATE TRIGGER queue_consumer_state_identity_guard
BEFORE UPDATE OF consumer_id, queue_id, worker_id ON queue_consumer_state
BEGIN
  SELECT RAISE(ABORT, 'queue consumer projection identity is immutable');
END;
CREATE TRIGGER queue_consumer_state_generation_guard
BEFORE UPDATE ON queue_consumer_state
WHEN OLD.consumer_generation != NEW.consumer_generation AND NOT (
  NEW.consumer_generation = OLD.consumer_generation + 1 AND
  OLD.state IN ('accepting', 'paused', 'draining') AND NEW.state = 'draining'
)
BEGIN
  SELECT RAISE(ABORT, 'queue consumer projection generation invariant');
END;
CREATE TRIGGER queue_consumer_state_digest_guard
BEFORE UPDATE ON queue_consumer_state
WHEN OLD.consumer_generation = NEW.consumer_generation AND
     OLD.descriptor_sha256 != NEW.descriptor_sha256
BEGIN
  SELECT RAISE(ABORT, 'queue consumer projection digest conflict');
END;
CREATE TRIGGER queue_delivery_batches_insert_guard
BEFORE INSERT ON queue_delivery_batches
WHEN NOT EXISTS (
  SELECT 1 FROM queue_consumer_state c
  WHERE c.consumer_id = NEW.consumer_id AND c.queue_id = NEW.queue_id
    AND c.consumer_generation = NEW.consumer_generation
    AND c.version_id = NEW.version_id
    AND c.execution_generation = NEW.execution_generation
    AND c.entrypoint IS NEW.entrypoint AND c.state = 'accepting'
    AND NEW.message_count <= c.max_batch_size
)
BEGIN
  SELECT RAISE(ABORT, 'queue delivery batch authority invariant');
END;
CREATE TRIGGER queue_delivery_batches_update_guard
BEFORE UPDATE ON queue_delivery_batches
BEGIN
  SELECT RAISE(ABORT, 'queue delivery batch is immutable');
END;
CREATE TRIGGER queue_delivery_batches_delete_guard
BEFORE DELETE ON queue_delivery_batches
WHEN EXISTS (
  SELECT 1 FROM queue_messages m
  WHERE m.claim_batch_id = OLD.id AND m.state = 'claimed'
)
BEGIN
  SELECT RAISE(ABORT, 'queue delivery batch still has claimed messages');
END;
CREATE TRIGGER queue_messages_immutable_guard
BEFORE UPDATE ON queue_messages
WHEN OLD.seq != NEW.seq OR OLD.id != NEW.id OR OLD.queue_id != NEW.queue_id OR
     OLD.queue_generation != NEW.queue_generation OR
     OLD.enqueued_at_ms != NEW.enqueued_at_ms OR OLD.expires_at_ms != NEW.expires_at_ms OR
     OLD.content_type != NEW.content_type OR OLD.body != NEW.body OR OLD.body_bytes != NEW.body_bytes
BEGIN
  SELECT RAISE(ABORT, 'queue message immutable content invariant');
END;
CREATE TRIGGER queue_messages_transition_guard
BEFORE UPDATE ON queue_messages
BEGIN
  SELECT CASE WHEN NOT (
    (OLD.state = 'ready' AND NEW.state = 'claimed' AND
      OLD.attempts = NEW.attempts AND
      NEW.claim_batch_id IS NOT NULL AND NEW.consumer_id IS NOT NULL AND
      NEW.consumer_generation IS NOT NULL AND length(NEW.claim_token) = 32 AND
      NEW.claimed_at_ms IS NOT NULL AND NEW.claim_until_ms > NEW.claimed_at_ms AND EXISTS (
        SELECT 1 FROM queue_delivery_batches b
        WHERE b.id = NEW.claim_batch_id AND b.queue_id = NEW.queue_id
          AND b.consumer_id = NEW.consumer_id
          AND b.consumer_generation = NEW.consumer_generation
          AND b.claim_token = NEW.claim_token
          AND b.claimed_at_ms = NEW.claimed_at_ms
          AND b.claim_until_ms = NEW.claim_until_ms
      )
    ) OR
    (OLD.state = 'claimed' AND NEW.state = 'ready' AND
      NEW.claim_batch_id IS NULL AND NEW.consumer_id IS NULL AND
      NEW.consumer_generation IS NULL AND NEW.claim_token IS NULL AND
      NEW.claimed_at_ms IS NULL AND NEW.claim_until_ms IS NULL AND
      NEW.attempts BETWEEN OLD.attempts AND OLD.attempts + 1 AND EXISTS (
        SELECT 1 FROM queue_delivery_batches b
        WHERE b.id = OLD.claim_batch_id AND b.queue_id = OLD.queue_id
          AND b.consumer_id = OLD.consumer_id
          AND b.consumer_generation = OLD.consumer_generation
          AND b.claim_token = OLD.claim_token
      )
    )
  ) THEN RAISE(ABORT, 'queue message transition invariant') END;
END;
CREATE TRIGGER queue_messages_claim_shape_insert_guard
BEFORE INSERT ON queue_messages
WHEN NEW.claim_batch_id IS NOT NULL OR NEW.consumer_id IS NOT NULL OR
     NEW.consumer_generation IS NOT NULL
BEGIN
  SELECT RAISE(ABORT, 'queue producer inserted claim authority');
END;
CREATE TRIGGER queue_messages_claim_shape_delete_guard
BEFORE DELETE ON queue_messages
WHEN OLD.state = 'claimed' AND NOT EXISTS (
  SELECT 1 FROM queue_delivery_batches b
  WHERE b.id = OLD.claim_batch_id AND b.consumer_id = OLD.consumer_id
    AND b.consumer_generation = OLD.consumer_generation AND b.claim_token = OLD.claim_token
)
BEGIN
  SELECT RAISE(ABORT, 'queue claimed message delete invariant');
END;
CREATE TRIGGER queue_dlq_pending_insert_guard
BEFORE INSERT ON queue_dlq_pending
WHEN NOT EXISTS (
  SELECT 1 FROM queue_messages m JOIN queue_state q ON q.queue_id = NEW.target_queue_id
  WHERE m.id = NEW.message_id AND m.queue_id = NEW.source_queue_id AND m.state = 'ready'
    AND m.attempts = NEW.terminal_attempts
    AND m.claim_token IS NULL AND m.claim_batch_id IS NULL AND m.consumer_id IS NULL
    AND q.lifecycle_generation = NEW.target_queue_generation
)
BEGIN
  SELECT RAISE(ABORT, 'queue DLQ pending authority invariant');
END;
CREATE TRIGGER queue_dlq_pending_update_guard
BEFORE UPDATE ON queue_dlq_pending
WHEN OLD.message_id != NEW.message_id OR OLD.source_queue_id != NEW.source_queue_id OR
     OLD.target_queue_id != NEW.target_queue_id OR
     OLD.target_queue_generation != NEW.target_queue_generation OR
     OLD.terminal_attempts != NEW.terminal_attempts OR OLD.created_at_ms != NEW.created_at_ms
BEGIN
  SELECT RAISE(ABORT, 'queue DLQ pending identity is immutable');
END;
CREATE TABLE cron_schedules (
  activation_id          TEXT PRIMARY KEY
                         CHECK(length(activation_id) = 36 AND activation_id = lower(activation_id)),
  account_id             TEXT NOT NULL
                         CHECK(length(account_id) = 36 AND account_id = lower(account_id)),
  worker_id              TEXT NOT NULL
                         CHECK(length(worker_id) = 36 AND worker_id = lower(worker_id)),
  version_id          TEXT NOT NULL
                         CHECK(length(version_id) = 36 AND version_id = lower(version_id)),
  execution_generation   INTEGER NOT NULL CHECK(execution_generation >= 1),
  activation_generation  INTEGER NOT NULL CHECK(activation_generation >= 1),
  expression             TEXT NOT NULL CHECK(length(expression) BETWEEN 1 AND 256),
  expression_sha256      BLOB NOT NULL CHECK(length(expression_sha256) = 32),
  parser_version         INTEGER NOT NULL CHECK(parser_version >= 1),
  state                  TEXT NOT NULL CHECK(state IN (
                           'staged', 'accepting', 'draining', 'deleting'
                         )),
  next_fire_at_ms        INTEGER NOT NULL CHECK(next_fire_at_ms >= 0),
  updated_at_ms          INTEGER NOT NULL
) STRICT;
CREATE INDEX cron_schedules_due
ON cron_schedules(state, next_fire_at_ms, activation_id);
CREATE TABLE cron_runs (
  id                    TEXT PRIMARY KEY
                        CHECK(length(id) = 36 AND id = lower(id)),
  activation_id         TEXT NOT NULL REFERENCES cron_schedules(activation_id),
  activation_generation INTEGER NOT NULL CHECK(activation_generation >= 1),
  scheduled_at_ms       INTEGER NOT NULL CHECK(scheduled_at_ms >= 0),
  version_id         TEXT NOT NULL
                        CHECK(length(version_id) = 36 AND version_id = lower(version_id)),
  execution_generation  INTEGER NOT NULL CHECK(execution_generation >= 1),
  expression            TEXT NOT NULL CHECK(length(expression) BETWEEN 1 AND 256),
  state                 TEXT NOT NULL CHECK(state IN (
                          'ready', 'claimed', 'complete', 'failed', 'skipped'
                        )),
  attempt               INTEGER NOT NULL DEFAULT 0 CHECK(attempt BETWEEN 0 AND 3),
  no_retry              INTEGER NOT NULL DEFAULT 0 CHECK(no_retry IN (0, 1)),
  next_attempt_at_ms    INTEGER,
  claim_token           BLOB,
  claimed_at_ms         INTEGER,
  claim_until_ms        INTEGER,
  error_code            TEXT CHECK(error_code IS NULL OR length(error_code) BETWEEN 1 AND 128),
  created_at_ms         INTEGER NOT NULL,
  completed_at_ms       INTEGER,
  UNIQUE(activation_id, activation_generation, scheduled_at_ms),
  CHECK(
    (state = 'ready' AND next_attempt_at_ms IS NOT NULL AND claim_token IS NULL AND
      claimed_at_ms IS NULL AND claim_until_ms IS NULL AND completed_at_ms IS NULL) OR
    (state = 'claimed' AND next_attempt_at_ms IS NULL AND length(claim_token) = 32 AND
      claimed_at_ms IS NOT NULL AND claim_until_ms > claimed_at_ms AND completed_at_ms IS NULL) OR
    (state IN ('complete', 'failed', 'skipped') AND next_attempt_at_ms IS NULL AND
      claim_token IS NULL AND claimed_at_ms IS NULL AND claim_until_ms IS NULL AND
      completed_at_ms IS NOT NULL)
  )
) STRICT;
CREATE INDEX cron_runs_due
ON cron_runs(state, next_attempt_at_ms, scheduled_at_ms, id)
WHERE state = 'ready';
CREATE INDEX cron_runs_expired
ON cron_runs(claim_until_ms, id)
WHERE state = 'claimed';
CREATE TRIGGER cron_schedules_identity_guard
BEFORE UPDATE OF activation_id, account_id, worker_id, version_id, execution_generation,
  activation_generation,
  expression, expression_sha256, parser_version ON cron_schedules
BEGIN
  SELECT RAISE(ABORT, 'cron schedule identity is immutable');
END;
CREATE TRIGGER cron_schedules_generation_digest_guard
BEFORE UPDATE ON cron_schedules
WHEN OLD.activation_generation = NEW.activation_generation AND
     OLD.expression_sha256 != NEW.expression_sha256
BEGIN
  SELECT RAISE(ABORT, 'cron schedule digest conflict');
END;
CREATE TRIGGER cron_schedules_next_fire_guard
BEFORE UPDATE OF next_fire_at_ms ON cron_schedules
WHEN NEW.next_fire_at_ms <= OLD.next_fire_at_ms
BEGIN
  SELECT RAISE(ABORT, 'cron schedule next fire must advance');
END;
CREATE TRIGGER cron_runs_insert_guard
BEFORE INSERT ON cron_runs
WHEN NEW.state != 'ready' OR NEW.attempt != 0 OR NEW.no_retry != 0 OR
     NOT EXISTS (
       SELECT 1 FROM cron_schedules s
       WHERE s.activation_id = NEW.activation_id
         AND s.activation_generation = NEW.activation_generation
         AND s.version_id = NEW.version_id
         AND s.execution_generation = NEW.execution_generation
         AND s.expression = NEW.expression AND s.state = 'accepting'
     )
BEGIN
  SELECT RAISE(ABORT, 'cron run insert authority invariant');
END;
CREATE TRIGGER cron_runs_identity_guard
BEFORE UPDATE ON cron_runs
WHEN OLD.id != NEW.id OR OLD.activation_id != NEW.activation_id OR
     OLD.activation_generation != NEW.activation_generation OR
     OLD.scheduled_at_ms != NEW.scheduled_at_ms OR
     OLD.version_id != NEW.version_id OR
     OLD.execution_generation != NEW.execution_generation OR
     OLD.expression != NEW.expression OR OLD.created_at_ms != NEW.created_at_ms
BEGIN
  SELECT RAISE(ABORT, 'cron run identity is immutable');
END;
CREATE TRIGGER cron_runs_transition_guard
BEFORE UPDATE ON cron_runs
WHEN NOT (
  (OLD.state = 'ready' AND NEW.state = 'claimed' AND
    NEW.attempt = OLD.attempt AND NEW.no_retry = OLD.no_retry) OR
  (OLD.state = 'claimed' AND NEW.state = 'ready' AND
    NEW.attempt BETWEEN OLD.attempt AND OLD.attempt + 1 AND
    NEW.no_retry = OLD.no_retry) OR
  (OLD.state = 'claimed' AND NEW.state IN ('complete', 'failed', 'skipped') AND
    NEW.attempt BETWEEN OLD.attempt AND OLD.attempt + 1) OR
  (OLD.state = NEW.state AND OLD.state IN ('complete', 'failed', 'skipped') AND
    OLD.id = NEW.id)
)
BEGIN
  SELECT RAISE(ABORT, 'cron run transition invariant');
END;
CREATE TABLE workflow_events (
  instance_id TEXT NOT NULL,
  instance_generation INTEGER NOT NULL,
  event_seq INTEGER NOT NULL CHECK(event_seq>=1),
  type TEXT NOT NULL CHECK(length(CAST(type AS BLOB)) BETWEEN 1 AND 100),
  payload_base64 BLOB NOT NULL CHECK(length(payload_base64)<=1398112),
  accepted_at_ms INTEGER NOT NULL,
  logical_bytes INTEGER NOT NULL CHECK(logical_bytes=length(CAST(type AS BLOB))+length(payload_base64)+32),
  PRIMARY KEY(instance_id,instance_generation,event_seq),
  FOREIGN KEY(instance_id,instance_generation) REFERENCES workflow_instances(id,instance_generation)
) WITHOUT ROWID, STRICT;
CREATE TABLE workflow_event_receipts (
  operation_id TEXT PRIMARY KEY,
  instance_id TEXT NOT NULL,
  instance_generation INTEGER NOT NULL CHECK(instance_generation>=1),
  type TEXT NOT NULL,
  payload_sha256 BLOB NOT NULL CHECK(length(payload_sha256)=32),
  accepted_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX workflow_event_receipts_instance ON workflow_event_receipts(instance_id,instance_generation);
CREATE TRIGGER workflow_event_receipt_immutable BEFORE UPDATE ON workflow_event_receipts
BEGIN SELECT RAISE(ABORT,'workflow event receipt is immutable'); END;
CREATE TABLE workflow_gc_receipts (
  operation_id TEXT PRIMARY KEY,
  instance_id TEXT NOT NULL UNIQUE,
  creation_nonce BLOB NOT NULL CHECK(length(creation_nonce)=32),
  creation_operation_id TEXT NOT NULL UNIQUE,
  instance_generation INTEGER NOT NULL CHECK(instance_generation>=1),
  deleted_at_ms INTEGER NOT NULL
) STRICT;
CREATE TABLE workflow_instances (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL,
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
  CHECK(success_retention_ms BETWEEN 3600000 AND 31536000000),
  CHECK(error_retention_ms BETWEEN 3600000 AND 31536000000),
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
CREATE TABLE workflow_mutation_context (
  instance_id TEXT PRIMARY KEY,
  operation_id TEXT NOT NULL UNIQUE,
  creation_nonce BLOB NOT NULL CHECK(length(creation_nonce)=32),
  expected_generation INTEGER NOT NULL CHECK(expected_generation>=1),
  target_generation INTEGER NOT NULL CHECK(target_generation>=1),
  kind TEXT NOT NULL CHECK(kind IN ('restart','purge','acknowledge_purge')),
  restart_from_name TEXT,
  restart_from_count INTEGER,
  restart_from_kind TEXT,
  restart_target_ordinal INTEGER,
  restart_retain_step_count INTEGER,
  restart_next_event_seq INTEGER,
  authorized_at_ms INTEGER NOT NULL,
  CHECK((kind='restart' AND expected_generation<9223372036854775807 AND target_generation=expected_generation+1)
    OR (kind IN ('purge','acknowledge_purge') AND target_generation=expected_generation)),
  CHECK((kind IN ('purge','acknowledge_purge') AND restart_from_name IS NULL AND restart_from_count IS NULL
      AND restart_from_kind IS NULL AND restart_target_ordinal IS NULL AND restart_retain_step_count IS NULL
      AND restart_next_event_seq IS NULL)
    OR (kind='restart' AND restart_retain_step_count BETWEEN 0 AND 1024 AND restart_next_event_seq>=1
      AND ((restart_from_name IS NULL AND restart_from_count IS NULL AND restart_from_kind IS NULL
          AND restart_target_ordinal IS NULL AND restart_retain_step_count=0 AND restart_next_event_seq=1)
        OR (length(CAST(restart_from_name AS BLOB)) BETWEEN 1 AND 256 AND restart_from_count BETWEEN 1 AND 1024
          AND (restart_from_kind IS NULL OR restart_from_kind IN ('do','sleep','waitForEvent'))
          AND restart_target_ordinal BETWEEN 0 AND 1023 AND restart_retain_step_count>restart_target_ordinal))))
) STRICT;
CREATE TABLE workflow_operation_progress (
  instance_id TEXT PRIMARY KEY,
  operation_id TEXT NOT NULL UNIQUE,
  operation_sequence INTEGER NOT NULL CHECK(operation_sequence>=1),
  creation_nonce BLOB NOT NULL CHECK(length(creation_nonce)=32),
  expected_generation INTEGER NOT NULL CHECK(expected_generation>=1),
  target_generation INTEGER NOT NULL CHECK(target_generation>=1),
  kind TEXT NOT NULL CHECK(kind IN ('restart','purge')),
  restart_from_name TEXT,
  restart_from_count INTEGER,
  restart_from_kind TEXT,
  restart_target_ordinal INTEGER,
  outcome TEXT NOT NULL CHECK(outcome IN ('applied','rejected')),
  error_code TEXT,
  decided_at_ms INTEGER NOT NULL,
  CHECK((kind='restart' AND expected_generation<9223372036854775807 AND target_generation=expected_generation+1)
    OR (kind='purge' AND target_generation=expected_generation)),
  CHECK((outcome='applied' AND error_code IS NULL) OR
    (outcome='rejected' AND error_code IN ('WORKFLOW_INSTANCE_NOT_FOUND','WORKFLOW_INSTANCE_STATE_CONFLICT','WORKFLOW_STATE_QUOTA_EXCEEDED'))),
  CHECK((kind='purge' AND restart_from_name IS NULL AND restart_from_count IS NULL
      AND restart_from_kind IS NULL AND restart_target_ordinal IS NULL)
    OR (kind='restart' AND restart_from_name IS NULL AND restart_from_count IS NULL
      AND restart_from_kind IS NULL AND restart_target_ordinal IS NULL)
    OR (kind='restart' AND length(CAST(restart_from_name AS BLOB)) BETWEEN 1 AND 256
      AND restart_from_count BETWEEN 1 AND 1024
      AND (restart_from_kind IS NULL OR restart_from_kind IN ('do','sleep','waitForEvent'))
      AND (restart_target_ordinal IS NULL OR restart_target_ordinal BETWEEN 0 AND 1023))),
  CHECK(outcome!='applied' OR restart_from_name IS NULL OR restart_target_ordinal IS NOT NULL)
) STRICT;
CREATE TABLE workflow_step_dependencies (
  instance_id TEXT NOT NULL,
  instance_generation INTEGER NOT NULL,
  child_ordinal INTEGER NOT NULL,
  parent_ordinal INTEGER NOT NULL CHECK(parent_ordinal<child_ordinal),
  PRIMARY KEY(instance_id,instance_generation,child_ordinal,parent_ordinal),
  FOREIGN KEY(instance_id,instance_generation,child_ordinal) REFERENCES workflow_steps(instance_id,instance_generation,ordinal),
  FOREIGN KEY(instance_id,instance_generation,parent_ordinal) REFERENCES workflow_steps(instance_id,instance_generation,ordinal)
) WITHOUT ROWID, STRICT;
CREATE TABLE workflow_steps (
  instance_id TEXT NOT NULL,
  instance_generation INTEGER NOT NULL CHECK(instance_generation>=1),
  ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 1023),
  name TEXT NOT NULL CHECK(length(CAST(name AS BLOB)) BETWEEN 1 AND 256),
  name_count INTEGER NOT NULL CHECK(name_count>0),
  kind TEXT NOT NULL CHECK(kind IN ('do','sleep','sleep_until','wait_event')),
  config_json BLOB NOT NULL CHECK(length(config_json)<=4096),
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256)=32),
  state TEXT NOT NULL CHECK(state IN ('pending','running','delay_pending','retry_wait','waiting','complete','failed','cancelled')),
  attempt INTEGER NOT NULL CHECK(attempt BETWEEN 0 AND 101),
  run_token BLOB,
  step_token BLOB,
  output_json BLOB CHECK(output_json IS NULL OR length(output_json)<=CASE WHEN kind='wait_event' THEN 1400160 ELSE 1398112 END),
  error_json BLOB CHECK(error_json IS NULL OR length(error_json)<=8192),
  error_code TEXT,
  started_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  completed_at_ms INTEGER,
  config_sha256 BLOB NOT NULL CHECK(length(config_sha256)=32),
  batch_first_ordinal INTEGER NOT NULL DEFAULT 0 CHECK(batch_first_ordinal BETWEEN 0 AND 1023),
  batch_size INTEGER NOT NULL DEFAULT 1 CHECK(batch_size BETWEEN 1 AND 16),
  dependency_count INTEGER NOT NULL DEFAULT 0 CHECK(dependency_count BETWEEN 0 AND 16),
  attempt_started_at_ms INTEGER,
  attempt_deadline_at_ms INTEGER,
  due_at_ms INTEGER,
  retry_delay_ms INTEGER,
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
  CHECK((retry_delay_ms IS NOT NULL)=(state='retry_wait')),
  CHECK(retry_delay_ms IS NULL OR (retry_delay_ms BETWEEN 0 AND 86400000 AND due_at_ms=updated_at_ms+retry_delay_ms)),
  CHECK(state!='retry_wait' OR (kind='do' AND error_json IS NOT NULL AND error_code IS NOT NULL)),
  CHECK(state!='waiting' OR kind!='do'),
  CHECK(state!='failed' OR (error_json IS NOT NULL AND error_code IS NOT NULL)),
  CHECK(state IN ('failed','delay_pending','retry_wait') OR (error_json IS NULL AND error_code IS NULL)),
  CHECK((output_json IS NOT NULL)=(state='complete' AND kind IN ('do','wait_event'))),
  CHECK(kind='do' OR attempt=0),
  CHECK((attempt_started_at_ms IS NULL)=(attempt_deadline_at_ms IS NULL)),
  CHECK(attempt_deadline_at_ms IS NULL OR attempt_deadline_at_ms>attempt_started_at_ms),
  CHECK(event_buffer_ceiling IS NULL OR (kind='wait_event' AND event_buffer_ceiling>=0)),
  CHECK(consumed_event_seq IS NULL OR (kind='wait_event' AND state='complete' AND consumed_event_seq>=1)),
  CHECK(json_valid(CAST(config_json AS TEXT)) AND json_type(CAST(config_json AS TEXT))='object'),
  CHECK(json_type(CAST(config_json AS TEXT),'$.rollbackStep') IN ('true','false')),
  CHECK(ordinal>=batch_first_ordinal AND ordinal<batch_first_ordinal+batch_size AND batch_first_ordinal+batch_size<=1024),
  CHECK(kind!='do' OR ((attempt=0 AND state IN ('pending','cancelled') AND attempt_started_at_ms IS NULL)
    OR (attempt>=1 AND attempt_started_at_ms IS NOT NULL))),
  CHECK(kind='do' OR (attempt_started_at_ms IS NULL AND state IN ('waiting','complete','failed','cancelled'))),
  CHECK(kind='wait_event' OR event_buffer_ceiling IS NULL),
  CHECK(kind!='wait_event' OR (event_buffer_ceiling IS NOT NULL AND (state!='complete' OR consumed_event_seq IS NOT NULL)))
) WITHOUT ROWID, STRICT;
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
CREATE INDEX workflow_events_fifo ON workflow_events(instance_id,instance_generation,type,event_seq);
CREATE INDEX workflow_instances_account ON workflow_instances(account_id,definition_id,state);
CREATE INDEX workflow_instances_due ON workflow_instances(next_run_at_ms,created_at_ms,id) WHERE state='queued';
CREATE INDEX workflow_instances_expired ON workflow_instances(run_lease_until_ms,id) WHERE state='running';
CREATE INDEX workflow_instances_fair ON workflow_instances(has_activated,account_id,next_run_at_ms,created_at_ms,id)
  WHERE state='queued';
CREATE INDEX workflow_instances_retention ON workflow_instances(expires_at_ms,id) WHERE capability_version=1 AND state IN ('complete','errored','terminated');
CREATE INDEX workflow_instances_waiting ON workflow_instances(next_wake_at_ms,id) WHERE state='waiting';
CREATE INDEX workflow_steps_pending_timeout ON workflow_steps(attempt_deadline_at_ms,instance_id,ordinal)
  WHERE state='pending' AND attempt>0;
CREATE INDEX workflow_steps_delay_pending ON workflow_steps(updated_at_ms,instance_id,ordinal)
  WHERE state='delay_pending';
CREATE INDEX workflow_steps_retry_due ON workflow_steps(due_at_ms,instance_id,ordinal)
  WHERE state='retry_wait';
CREATE INDEX workflow_steps_wait_due ON workflow_steps(due_at_ms,instance_id,ordinal)
  WHERE state='waiting';
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
CREATE TRIGGER workflow_context_immutable BEFORE UPDATE ON workflow_mutation_context
BEGIN SELECT RAISE(ABORT,'workflow operation context is immutable'); END;
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
CREATE TRIGGER workflow_dependency_immutable BEFORE UPDATE ON workflow_step_dependencies
BEGIN SELECT RAISE(ABORT,'workflow dependency is immutable'); END;
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
CREATE TRIGGER workflow_event_immutable BEFORE UPDATE ON workflow_events
BEGIN SELECT RAISE(ABORT,'workflow event is immutable'); END;
CREATE TRIGGER workflow_event_insert_guard BEFORE INSERT ON workflow_events
WHEN NEW.type GLOB '*[^A-Za-z0-9_-]*' OR NEW.type GLOB '-*'
  OR NOT EXISTS(SELECT 1 FROM workflow_instances i WHERE i.id=NEW.instance_id AND i.instance_generation=NEW.instance_generation
    AND i.capability_version=1 AND i.state IN ('queued','running','waiting','paused') AND i.next_event_seq=NEW.event_seq
    AND i.next_event_seq<9223372036854775807)
BEGIN SELECT RAISE(ABORT,'workflow event intake fence'); END;
CREATE TRIGGER workflow_progress_acknowledge_guard BEFORE DELETE ON workflow_mutation_context
WHEN OLD.kind='acknowledge_purge' AND EXISTS(SELECT 1 FROM workflow_operation_progress WHERE instance_id=OLD.instance_id)
BEGIN SELECT RAISE(ABORT,'workflow purge watermark is not swept'); END;
CREATE TRIGGER workflow_progress_delete_guard BEFORE DELETE ON workflow_operation_progress
WHEN OLD.outcome!='applied' OR OLD.kind!='purge' OR NOT EXISTS(SELECT 1 FROM workflow_mutation_context c
  WHERE c.kind='acknowledge_purge' AND c.instance_id=OLD.instance_id AND c.operation_id=OLD.operation_id
    AND c.creation_nonce=OLD.creation_nonce AND c.expected_generation=OLD.expected_generation)
BEGIN SELECT RAISE(ABORT,'workflow operation watermark requires acknowledged purge'); END;
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
CREATE TRIGGER workflow_progress_rejection_insert_guard BEFORE INSERT ON workflow_operation_progress
WHEN NEW.outcome='rejected' AND NEW.error_code IS NULL
BEGIN SELECT RAISE(ABORT,'workflow rejection requires a stable error code'); END;
CREATE TRIGGER workflow_progress_rejection_update_guard BEFORE UPDATE ON workflow_operation_progress
WHEN NEW.outcome='rejected' AND NEW.error_code IS NULL
BEGIN SELECT RAISE(ABORT,'workflow rejection requires a stable error code'); END;
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
CREATE TRIGGER workflow_receipt_delete_guard BEFORE DELETE ON workflow_gc_receipts
WHEN NOT EXISTS(SELECT 1 FROM workflow_mutation_context c WHERE c.kind='acknowledge_purge'
  AND c.operation_id=OLD.operation_id AND c.instance_id=OLD.instance_id AND c.creation_nonce=OLD.creation_nonce
  AND c.expected_generation=OLD.instance_generation)
BEGIN SELECT RAISE(ABORT,'workflow purge is not acknowledged'); END;
CREATE TRIGGER workflow_receipt_immutable BEFORE UPDATE ON workflow_gc_receipts
BEGIN SELECT RAISE(ABORT,'workflow purge receipt is immutable'); END;
CREATE TRIGGER workflow_receipt_insert_guard BEFORE INSERT ON workflow_gc_receipts
WHEN EXISTS(SELECT 1 FROM workflow_instances WHERE id=NEW.instance_id) OR NOT EXISTS(
  SELECT 1 FROM workflow_mutation_context c WHERE c.kind='purge' AND c.operation_id=NEW.operation_id
    AND c.instance_id=NEW.instance_id AND c.creation_nonce=NEW.creation_nonce AND c.expected_generation=NEW.instance_generation
    AND c.authorized_at_ms=NEW.deleted_at_ms)
BEGIN SELECT RAISE(ABORT,'workflow purge receipt requires exact deletion'); END;
CREATE TRIGGER workflow_step_extra_identity_guard BEFORE UPDATE OF config_sha256,batch_first_ordinal,batch_size,
  dependency_count,event_buffer_ceiling ON workflow_steps
BEGIN SELECT RAISE(ABORT,'workflow durable descriptor is immutable'); END;
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
CREATE TRIGGER workflow_instance_identity_guard BEFORE UPDATE OF id,account_id,definition_id,definition_name,
  external_instance_id,workflow_version_id,worker_id,worker_version_id,worker_code_sha256,class_name,creation_nonce,creation_operation_id,creation_batch_id,
  loader_schema_version,capability_version,descriptor_sha256,input_json,created_at_ms,success_retention_ms,error_retention_ms
ON workflow_instances WHEN OLD.capability_version=1
BEGIN SELECT RAISE(ABORT,'workflow durable identity is immutable'); END;
CREATE INDEX workflow_instance_creation_batch ON workflow_instances(creation_batch_id);
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
CREATE TRIGGER workflow_step_attempt_guard BEFORE UPDATE ON workflow_steps
WHEN OLD.config_sha256 IS NOT NULL AND (NEW.attempt!=OLD.attempt
  OR NEW.attempt_started_at_ms IS NOT OLD.attempt_started_at_ms OR NEW.attempt_deadline_at_ms IS NOT OLD.attempt_deadline_at_ms)
  AND NOT (NEW.state='running' AND NEW.kind='do' AND NEW.attempt=OLD.attempt+1
    AND ((OLD.state='pending' AND OLD.attempt=0) OR (OLD.state='retry_wait' AND OLD.due_at_ms<=NEW.updated_at_ms))
    AND NEW.attempt_started_at_ms=NEW.updated_at_ms
    AND NEW.attempt_deadline_at_ms=NEW.attempt_started_at_ms+json_extract(CAST(NEW.config_json AS TEXT),'$.timeout')
    AND NEW.attempt<=1+json_extract(CAST(NEW.config_json AS TEXT),'$.retries.limit'))
BEGIN SELECT RAISE(ABORT,'workflow durable business attempt'); END;
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
CREATE TRIGGER workflow_step_identity_guard BEFORE UPDATE OF instance_id,instance_generation,ordinal,name,name_count,
  kind,config_json,descriptor_sha256,started_at_ms ON workflow_steps WHEN OLD.config_sha256 IS NOT NULL
BEGIN SELECT RAISE(ABORT,'workflow durable step identity is immutable'); END;
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
CREATE TRIGGER workflow_step_terminal_guard BEFORE UPDATE ON workflow_steps
WHEN OLD.config_sha256 IS NOT NULL AND OLD.state IN ('complete','failed','cancelled')
BEGIN SELECT RAISE(ABORT,'workflow settled result is immutable'); END;
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
CREATE TRIGGER workflow_wait_settlement_guard BEFORE UPDATE ON workflow_steps
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
      AND json_extract(CAST(NEW.output_json AS TEXT),'$.payloadBase64')=CAST(e.payload_base64 AS TEXT))
    THEN RAISE(ABORT,'workflow event is not eligible') END;
  SELECT CASE WHEN NEW.state='failed' AND (OLD.kind!='wait_event' OR OLD.due_at_ms>NEW.updated_at_ms
    OR NEW.error_code!='WORKFLOW_EVENT_TIMEOUT' OR EXISTS(SELECT 1 FROM workflow_events e WHERE e.instance_id=OLD.instance_id
      AND e.instance_generation=OLD.instance_generation AND e.type=json_extract(CAST(OLD.config_json AS TEXT),'$.type')
      AND (e.accepted_at_ms<OLD.due_at_ms OR e.event_seq<=OLD.event_buffer_ceiling)))
    THEN RAISE(ABORT,'workflow event timeout arbitration') END;
END;
CREATE TABLE scheduler_meta (singleton INTEGER PRIMARY KEY CHECK(singleton=1), data_format TEXT NOT NULL, created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL) STRICT;
