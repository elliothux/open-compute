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
