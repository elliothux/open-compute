-- One process owns scheduler claims; interrupted deliveries are recovered before dispatch.

PRAGMA defer_foreign_keys=ON;

DROP TRIGGER queue_messages_insert_guard;

DROP TRIGGER queue_messages_counter_insert;

DROP TRIGGER queue_messages_counter_delete;

DROP TRIGGER queue_state_delete_guard;

DROP TRIGGER queue_messages_immutable_guard;

DROP TRIGGER queue_messages_claim_shape_insert_guard;

DROP TRIGGER queue_dlq_pending_insert_guard;

DROP TRIGGER queue_delivery_batches_insert_guard;

DROP TRIGGER queue_delivery_batches_update_guard;

DROP TRIGGER queue_delivery_batches_delete_guard;

DROP TRIGGER queue_messages_transition_guard;

DROP TRIGGER queue_messages_claim_shape_delete_guard;

DROP TRIGGER cron_runs_insert_guard;

DROP TRIGGER cron_runs_identity_guard;

DROP TRIGGER cron_runs_transition_guard;

DROP TRIGGER queue_delivery_batches_pin_insert;

DROP TRIGGER queue_delivery_batches_pin_delete;

DROP TRIGGER cron_runs_pin_insert;

DROP TRIGGER cron_runs_pin_delete;

DROP TRIGGER cron_runs_pin_terminal;

CREATE TABLE scheduled_jobs_next (
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
  last_error_code       TEXT,
  created_at_ms         INTEGER NOT NULL,
  updated_at_ms         INTEGER NOT NULL,
  CHECK(length(object_id) = 64 AND object_id = lower(object_id)),
  CHECK(object_id NOT GLOB '*[^0-9a-f]*'),
  CHECK(length(row_token) BETWEEN 16 AND 128),
  UNIQUE(namespace_resource_id, object_id, object_generation)
) STRICT;

INSERT INTO scheduled_jobs_next (id, kind, namespace_resource_id, object_id, object_generation, row_token, due_at_ms, target_version_id, execution_generation, state, retry_count, last_error_code, created_at_ms, updated_at_ms) SELECT id, kind, namespace_resource_id, object_id, object_generation, row_token, due_at_ms, target_version_id, execution_generation, state, retry_count, last_error_code, created_at_ms, updated_at_ms FROM scheduled_jobs;

DROP TABLE scheduled_jobs;

ALTER TABLE scheduled_jobs_next RENAME TO scheduled_jobs;

CREATE TABLE queue_messages_next (
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
  claim_batch_id       TEXT,
  consumer_id          TEXT,
  consumer_generation  INTEGER,
  CHECK(body_bytes = length(body)),
  CHECK(available_at_ms >= enqueued_at_ms),
  CHECK(expires_at_ms > enqueued_at_ms),
  CHECK((state = 'ready' AND claim_batch_id IS NULL AND consumer_id IS NULL AND consumer_generation IS NULL)
     OR (state = 'claimed' AND claim_batch_id IS NOT NULL AND consumer_id IS NOT NULL AND consumer_generation IS NOT NULL))
) STRICT;

INSERT INTO queue_messages_next (seq, id, queue_id, queue_generation, enqueued_at_ms, available_at_ms, expires_at_ms, content_type, body, body_bytes, state, attempts, claim_batch_id, consumer_id, consumer_generation) SELECT seq, id, queue_id, queue_generation, enqueued_at_ms, available_at_ms, expires_at_ms, content_type, body, body_bytes, state, attempts, claim_batch_id, consumer_id, consumer_generation FROM queue_messages;

-- Preserve ordering even when the highest sequence was already deleted.
UPDATE sqlite_sequence SET seq=MAX(seq,COALESCE((SELECT seq FROM sqlite_sequence WHERE name='queue_messages'),0))
WHERE name='queue_messages_next';

DROP TABLE queue_messages;

ALTER TABLE queue_messages_next RENAME TO queue_messages;

CREATE TABLE queue_delivery_batches_next (
  id                    TEXT PRIMARY KEY
                        CHECK(length(id) = 36 AND id = lower(id)),
  queue_id              TEXT NOT NULL REFERENCES queue_state(queue_id),
  consumer_id           TEXT NOT NULL,
  consumer_generation   INTEGER NOT NULL CHECK(consumer_generation >= 1),
  version_id         TEXT NOT NULL
                        CHECK(length(version_id) = 36 AND version_id = lower(version_id)),
  execution_generation  INTEGER NOT NULL CHECK(execution_generation >= 1),
  entrypoint            TEXT CHECK(entrypoint IS NULL OR length(entrypoint) BETWEEN 1 AND 128),
  state                 TEXT NOT NULL CHECK(state = 'claimed'),
  message_count         INTEGER NOT NULL CHECK(message_count BETWEEN 1 AND 100),
  created_at_ms         INTEGER NOT NULL
) STRICT;

INSERT INTO queue_delivery_batches_next (id, queue_id, consumer_id, consumer_generation, version_id, execution_generation, entrypoint, state, message_count, created_at_ms) SELECT id, queue_id, consumer_id, consumer_generation, version_id, execution_generation, entrypoint, state, message_count, created_at_ms FROM queue_delivery_batches;

DROP TABLE queue_delivery_batches;

ALTER TABLE queue_delivery_batches_next RENAME TO queue_delivery_batches;

CREATE TABLE cron_runs_next (
  id                     TEXT PRIMARY KEY
                         CHECK(length(id) = 36 AND id = lower(id)),
  activation_id          TEXT NOT NULL,
  worker_id TEXT NOT NULL,
  scheduled_at_ms        INTEGER NOT NULL CHECK(scheduled_at_ms >= 0),
  version_id             TEXT NOT NULL
                         CHECK(length(version_id) = 36 AND version_id = lower(version_id)),
  execution_generation   INTEGER NOT NULL CHECK(execution_generation >= 1),
  expression             TEXT NOT NULL CHECK(length(expression) BETWEEN 1 AND 256),
  state                  TEXT NOT NULL CHECK(state IN (
                           'ready', 'claimed', 'complete', 'failed', 'skipped'
                         )),
  attempt                INTEGER NOT NULL DEFAULT 0 CHECK(attempt BETWEEN 0 AND 4),
  no_retry               INTEGER NOT NULL DEFAULT 0 CHECK(no_retry IN (0, 1)),
  next_attempt_at_ms     INTEGER,
  claimed_at_ms          INTEGER,
  first_dispatched_at_ms INTEGER,
  dispatch_deadline_at_ms INTEGER,
  last_unknown_reason    TEXT CHECK(last_unknown_reason IS NULL OR last_unknown_reason IN (
                           'transport-timeout', 'connection-loss', 'malformed-response',
                           'runtime-generation-lost'
                         )),
  error_code             TEXT CHECK(error_code IS NULL OR length(error_code) BETWEEN 1 AND 128),
  created_at_ms          INTEGER NOT NULL,
  completed_at_ms        INTEGER,
  UNIQUE(activation_id, scheduled_at_ms),
  CHECK((first_dispatched_at_ms IS NULL AND dispatch_deadline_at_ms IS NULL AND attempt = 0) OR
        (first_dispatched_at_ms IS NOT NULL AND dispatch_deadline_at_ms = first_dispatched_at_ms + 900000)),
  CHECK(
    (state = 'ready' AND next_attempt_at_ms IS NOT NULL AND
      claimed_at_ms IS NULL AND completed_at_ms IS NULL) OR
    (state = 'claimed' AND next_attempt_at_ms IS NULL AND
      claimed_at_ms IS NOT NULL AND
      completed_at_ms IS NULL) OR
    (state IN ('complete', 'failed', 'skipped') AND next_attempt_at_ms IS NULL AND
      claimed_at_ms IS NULL AND
      completed_at_ms IS NOT NULL)
  )
) STRICT;

INSERT INTO cron_runs_next (id, activation_id, worker_id, scheduled_at_ms, version_id, execution_generation, expression, state, attempt, no_retry, next_attempt_at_ms, claimed_at_ms, first_dispatched_at_ms, dispatch_deadline_at_ms, last_unknown_reason, error_code, created_at_ms, completed_at_ms) SELECT id, activation_id, worker_id, scheduled_at_ms, version_id, execution_generation, expression, state, attempt, no_retry, next_attempt_at_ms, claimed_at_ms, first_dispatched_at_ms, dispatch_deadline_at_ms, last_unknown_reason, error_code, created_at_ms, completed_at_ms FROM cron_runs;

DROP TABLE cron_runs;

ALTER TABLE cron_runs_next RENAME TO cron_runs;

CREATE INDEX scheduled_jobs_due
ON scheduled_jobs(due_at_ms, id)
WHERE state = 'scheduled';

CREATE INDEX scheduled_jobs_discarding
ON scheduled_jobs(updated_at_ms, id)
WHERE state = 'discarding';

CREATE INDEX queue_messages_due
ON queue_messages(queue_id, state, available_at_ms, seq);

CREATE INDEX queue_messages_retention
ON queue_messages(expires_at_ms, queue_id, seq);

CREATE INDEX queue_messages_oldest
ON queue_messages(queue_id, enqueued_at_ms, seq);

CREATE INDEX queue_messages_claimed_batch
ON queue_messages(claim_batch_id, seq)
WHERE state = 'claimed';

CREATE INDEX queue_messages_batch_eligibility
ON queue_messages(queue_id, available_at_ms, seq)
WHERE state = 'ready';

CREATE INDEX queue_delivery_batches_consumer
ON queue_delivery_batches(consumer_id, consumer_generation, id);

CREATE INDEX cron_runs_due
ON cron_runs(state, next_attempt_at_ms, scheduled_at_ms, id)
WHERE state = 'ready';

UPDATE scheduled_jobs SET state='scheduled',updated_at_ms=CAST(unixepoch('subsec')*1000 AS INTEGER) WHERE state='claimed';
UPDATE queue_messages SET state='ready',attempts=attempts+1,available_at_ms=MAX(enqueued_at_ms,CAST(unixepoch('subsec')*1000 AS INTEGER)+250),
  claim_batch_id=NULL,consumer_id=NULL,consumer_generation=NULL WHERE state='claimed';
DELETE FROM queue_delivery_batches;
UPDATE cron_runs SET state=CASE WHEN attempt<3+1 AND dispatch_deadline_at_ms>CAST(unixepoch('subsec')*1000 AS INTEGER)+250 THEN 'ready' ELSE 'failed' END,
  next_attempt_at_ms=CASE WHEN attempt<3+1 AND dispatch_deadline_at_ms>CAST(unixepoch('subsec')*1000 AS INTEGER)+250 THEN CAST(unixepoch('subsec')*1000 AS INTEGER)+250 ELSE NULL END,
  claimed_at_ms=NULL,last_unknown_reason=COALESCE(last_unknown_reason,'transport-timeout'),
  error_code=CASE WHEN dispatch_deadline_at_ms<=CAST(unixepoch('subsec')*1000 AS INTEGER)+250 THEN 'CRON_DISPATCH_DEADLINE'
    WHEN attempt>=3+1 THEN 'CRON_RETRY_EXHAUSTED' ELSE error_code END,
  completed_at_ms=CASE WHEN attempt<3+1 AND dispatch_deadline_at_ms>CAST(unixepoch('subsec')*1000 AS INTEGER)+250 THEN NULL ELSE CAST(unixepoch('subsec')*1000 AS INTEGER) END
WHERE state='claimed';

CREATE TRIGGER queue_messages_insert_guard
BEFORE INSERT ON queue_messages
BEGIN
  SELECT CASE WHEN NEW.state != 'ready' OR NEW.attempts != 0 OR
                   NEW.claim_batch_id IS NOT NULL
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

CREATE TRIGGER queue_messages_immutable_guard
BEFORE UPDATE ON queue_messages
WHEN OLD.seq != NEW.seq OR OLD.id != NEW.id OR OLD.queue_id != NEW.queue_id OR
     OLD.queue_generation != NEW.queue_generation OR
     OLD.enqueued_at_ms != NEW.enqueued_at_ms OR OLD.expires_at_ms != NEW.expires_at_ms OR
     OLD.content_type != NEW.content_type OR OLD.body != NEW.body OR OLD.body_bytes != NEW.body_bytes
BEGIN
  SELECT RAISE(ABORT, 'queue message immutable content invariant');
END;

CREATE TRIGGER queue_messages_claim_shape_insert_guard
BEFORE INSERT ON queue_messages
WHEN NEW.claim_batch_id IS NOT NULL OR NEW.consumer_id IS NOT NULL OR
     NEW.consumer_generation IS NOT NULL
BEGIN
  SELECT RAISE(ABORT, 'queue producer inserted claim authority');
END;

CREATE TRIGGER queue_dlq_pending_insert_guard
BEFORE INSERT ON queue_dlq_pending
WHEN NOT EXISTS (
  SELECT 1 FROM queue_messages m JOIN queue_state q ON q.queue_id = NEW.target_queue_id
  WHERE m.id = NEW.message_id AND m.queue_id = NEW.source_queue_id AND m.state = 'ready'
    AND m.attempts = NEW.terminal_attempts
    AND m.claim_batch_id IS NULL AND m.consumer_id IS NULL
    AND q.lifecycle_generation = NEW.target_queue_generation
)
BEGIN
  SELECT RAISE(ABORT, 'queue DLQ pending authority invariant');
END;

CREATE TRIGGER queue_delivery_batches_insert_guard
BEFORE INSERT ON queue_delivery_batches
WHEN NOT EXISTS (
  SELECT 1 FROM queue_consumer_state c
  WHERE c.consumer_id = NEW.consumer_id AND c.queue_id = NEW.queue_id
    AND c.consumer_generation = NEW.consumer_generation
    AND c.version_id = NEW.version_id
    AND c.execution_generation = NEW.execution_generation
    AND c.entrypoint IS NEW.entrypoint AND c.state = 'active'
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

CREATE TRIGGER queue_messages_transition_guard
BEFORE UPDATE ON queue_messages
BEGIN
  SELECT CASE WHEN NOT (
    (OLD.state = 'ready' AND NEW.state = 'claimed' AND
      OLD.attempts = NEW.attempts AND
      NEW.claim_batch_id IS NOT NULL AND NEW.consumer_id IS NOT NULL AND
      NEW.consumer_generation IS NOT NULL AND
      EXISTS (
        SELECT 1 FROM queue_delivery_batches b
        WHERE b.id = NEW.claim_batch_id AND b.queue_id = NEW.queue_id
          AND b.consumer_id = NEW.consumer_id
          AND b.consumer_generation = NEW.consumer_generation



      )
    ) OR
    (OLD.state = 'claimed' AND NEW.state = 'ready' AND
      NEW.claim_batch_id IS NULL AND NEW.consumer_id IS NULL AND
      NEW.consumer_generation IS NULL AND
      NEW.attempts BETWEEN OLD.attempts AND OLD.attempts + 1 AND EXISTS (
        SELECT 1 FROM queue_delivery_batches b
        WHERE b.id = OLD.claim_batch_id AND b.queue_id = OLD.queue_id
          AND b.consumer_id = OLD.consumer_id
          AND b.consumer_generation = OLD.consumer_generation

      )
    )
  ) THEN RAISE(ABORT, 'queue message transition invariant') END;
END;

CREATE TRIGGER queue_messages_claim_shape_delete_guard
BEFORE DELETE ON queue_messages
WHEN OLD.state = 'claimed' AND NOT EXISTS (
  SELECT 1 FROM queue_delivery_batches b
  WHERE b.id = OLD.claim_batch_id AND b.consumer_id = OLD.consumer_id
    AND b.consumer_generation = OLD.consumer_generation
)
BEGIN
  SELECT RAISE(ABORT, 'queue claimed message delete invariant');
END;

CREATE TRIGGER cron_runs_insert_guard
BEFORE INSERT ON cron_runs
WHEN NEW.state != 'ready' OR NEW.attempt != 0 OR NEW.no_retry != 0 OR
     NEW.first_dispatched_at_ms IS NOT NULL OR NEW.dispatch_deadline_at_ms IS NOT NULL OR
     NEW.last_unknown_reason IS NOT NULL OR
     NOT EXISTS (
       SELECT 1 FROM cron_schedules s
       WHERE s.activation_id = NEW.activation_id
         AND s.version_id = NEW.version_id
         AND s.execution_generation = NEW.execution_generation
         AND s.expression = NEW.expression
     )
BEGIN
  SELECT RAISE(ABORT, 'cron run insert authority invariant');
END;

CREATE TRIGGER cron_runs_identity_guard
BEFORE UPDATE ON cron_runs
WHEN OLD.id != NEW.id OR OLD.activation_id != NEW.activation_id OR
     OLD.scheduled_at_ms != NEW.scheduled_at_ms OR
     OLD.version_id != NEW.version_id OR
     OLD.execution_generation != NEW.execution_generation OR
     OLD.expression != NEW.expression OR OLD.created_at_ms != NEW.created_at_ms OR
     (OLD.first_dispatched_at_ms IS NOT NULL AND
       (OLD.first_dispatched_at_ms != NEW.first_dispatched_at_ms OR
        OLD.dispatch_deadline_at_ms != NEW.dispatch_deadline_at_ms))
BEGIN
  SELECT RAISE(ABORT, 'cron run identity is immutable');
END;

CREATE TRIGGER cron_runs_transition_guard
BEFORE UPDATE ON cron_runs
WHEN NOT (
  (OLD.state = 'ready' AND NEW.state = 'claimed' AND
    NEW.attempt = OLD.attempt + 1 AND NEW.no_retry = OLD.no_retry AND
    NEW.first_dispatched_at_ms IS NOT NULL) OR
  (OLD.state = 'claimed' AND NEW.state = 'ready' AND
    NEW.attempt = OLD.attempt AND NEW.no_retry = OLD.no_retry) OR
  (OLD.state = 'claimed' AND NEW.state IN ('complete', 'failed', 'skipped') AND
    NEW.attempt = OLD.attempt) OR
  (OLD.state = 'ready' AND NEW.state IN ('failed', 'skipped') AND
    NEW.attempt = OLD.attempt) OR
  (OLD.state = NEW.state AND OLD.state = 'claimed' AND
    OLD.attempt = NEW.attempt) OR
  (OLD.state = NEW.state AND OLD.state IN ('complete', 'failed', 'skipped') AND
    OLD.id = NEW.id)
)
BEGIN
  SELECT RAISE(ABORT, 'cron run transition invariant');
END;

CREATE TRIGGER queue_delivery_batches_pin_insert AFTER INSERT ON queue_delivery_batches WHEN NEW.version_id IN (SELECT id FROM worker_versions) BEGIN INSERT INTO version_referrers VALUES(NEW.version_id,'queue_batch',NEW.id,NEW.created_at_ms); END;

CREATE TRIGGER queue_delivery_batches_pin_delete AFTER DELETE ON queue_delivery_batches BEGIN DELETE FROM version_referrers WHERE version_id=OLD.version_id AND kind='queue_batch' AND ref_id=OLD.id; END;

CREATE TRIGGER cron_runs_pin_insert AFTER INSERT ON cron_runs WHEN NEW.version_id IN (SELECT id FROM worker_versions) BEGIN INSERT INTO version_referrers VALUES(NEW.version_id,'cron_run',NEW.id,NEW.created_at_ms); END;

CREATE TRIGGER cron_runs_pin_delete AFTER DELETE ON cron_runs BEGIN DELETE FROM version_referrers WHERE version_id=OLD.version_id AND kind='cron_run' AND ref_id=OLD.id; END;

CREATE TRIGGER cron_runs_pin_terminal AFTER UPDATE OF state ON cron_runs
WHEN NEW.state IN ('complete','failed','skipped') BEGIN
  DELETE FROM version_referrers WHERE kind='cron_run' AND ref_id=NEW.id;
END;

INSERT OR IGNORE INTO queue_dlq_pending
  (message_id,source_queue_id,target_queue_id,target_queue_generation,terminal_attempts,next_attempt_at_ms,created_at_ms,last_error_code)
SELECT m.id,m.queue_id,c.dlq_queue_id,c.dlq_queue_generation,m.attempts,CAST(unixepoch('subsec')*1000 AS INTEGER)+250,CAST(unixepoch('subsec')*1000 AS INTEGER),'QUEUE_DLQ_BACKPRESSURED'
FROM queue_messages m JOIN queue_consumer_state c ON c.queue_id=m.queue_id
WHERE m.state='ready' AND m.attempts>c.max_retries AND c.dlq_queue_id IS NOT NULL;
DELETE FROM queue_messages WHERE state='ready' AND EXISTS(SELECT 1 FROM queue_consumer_state c
  WHERE c.queue_id=queue_messages.queue_id AND queue_messages.attempts>c.max_retries AND c.dlq_queue_id IS NULL);

DELETE FROM version_referrers WHERE kind='queue_batch';

DELETE FROM version_referrers WHERE kind='cron_run' AND ref_id IN (SELECT id FROM cron_runs WHERE state IN ('complete','failed','skipped'));
