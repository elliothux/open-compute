DROP TRIGGER queue_messages_update_guard;

ALTER TABLE queue_messages ADD COLUMN claim_batch_id TEXT;
ALTER TABLE queue_messages ADD COLUMN consumer_id TEXT;
ALTER TABLE queue_messages ADD COLUMN consumer_generation INTEGER;

CREATE TABLE queue_consumer_state (
  consumer_id                    TEXT PRIMARY KEY
                                 CHECK(length(consumer_id) = 36 AND consumer_id = lower(consumer_id)),
  queue_id                       TEXT NOT NULL UNIQUE REFERENCES queue_state(queue_id),
  consumer_generation            INTEGER NOT NULL CHECK(consumer_generation >= 1),
  deployment_id                  TEXT NOT NULL
                                 CHECK(length(deployment_id) = 36 AND deployment_id = lower(deployment_id)),
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
  deployment_id         TEXT NOT NULL
                        CHECK(length(deployment_id) = 36 AND deployment_id = lower(deployment_id)),
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
    AND c.deployment_id = NEW.deployment_id
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
