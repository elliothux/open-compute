-- P2.2 keeps Queue lifecycle separate from the frozen P0 resource CHECK graph.
CREATE TABLE queues (
  id                       TEXT PRIMARY KEY
                           CHECK(length(id) = 36 AND id = lower(id)),
  account_id               TEXT NOT NULL REFERENCES accounts(id),
  name                     TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 128),
  state                    TEXT NOT NULL CHECK(state IN (
                             'creating', 'ready', 'deleting', 'tombstoned'
                           )),
  availability             TEXT NOT NULL CHECK(availability IN (
                             'healthy', 'degraded', 'unavailable'
                           )),
  availability_code        TEXT,
  lifecycle_generation     INTEGER NOT NULL CHECK(lifecycle_generation >= 1),
  config_generation        INTEGER NOT NULL CHECK(config_generation >= 1),
  delivery_delay_seconds   INTEGER NOT NULL
                           CHECK(delivery_delay_seconds BETWEEN 0 AND 86400),
  retention_seconds        INTEGER NOT NULL
                           CHECK(retention_seconds BETWEEN 60 AND 1209600),
  max_message_bytes        INTEGER NOT NULL CHECK(max_message_bytes > 0),
  max_batch_messages       INTEGER NOT NULL CHECK(max_batch_messages > 0),
  max_batch_bytes          INTEGER NOT NULL CHECK(max_batch_bytes > 0),
  max_backlog_bytes        INTEGER NOT NULL CHECK(max_backlog_bytes > 0),
  created_at_ms            INTEGER NOT NULL,
  updated_at_ms            INTEGER NOT NULL,
  deleted_at_ms            INTEGER,
  CHECK(availability_code IS NULL OR
        length(availability_code) BETWEEN 1 AND 128),
  CHECK((availability = 'healthy') = (availability_code IS NULL)),
  CHECK((state = 'tombstoned') = (deleted_at_ms IS NOT NULL))
) STRICT;

CREATE UNIQUE INDEX queues_live_name
ON queues(account_id, name)
WHERE state != 'tombstoned';

CREATE INDEX queues_reconcile
ON queues(state, availability, updated_at_ms, id)
WHERE state IN ('creating', 'deleting') OR availability != 'healthy';

CREATE TABLE queue_producer_bindings (
  id                         TEXT PRIMARY KEY
                             CHECK(length(id) = 36 AND id = lower(id)),
  version_id              TEXT NOT NULL REFERENCES worker_versions(id),
  name                       TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 64),
  queue_id                   TEXT NOT NULL REFERENCES queues(id),
  queue_lifecycle_generation INTEGER NOT NULL CHECK(queue_lifecycle_generation >= 1),
  capability_version         INTEGER NOT NULL CHECK(capability_version >= 1),
  descriptor_sha256          BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms              INTEGER NOT NULL,
  UNIQUE(version_id, name)
) STRICT;

CREATE INDEX queue_producer_bindings_queue
ON queue_producer_bindings(queue_id, version_id, id);

CREATE TABLE queue_referrers (
  queue_id       TEXT NOT NULL REFERENCES queues(id),
  referrer_kind  TEXT NOT NULL CHECK(referrer_kind IN (
                   'producer_binding', 'consumer', 'dlq'
                 )),
  referrer_id    TEXT NOT NULL,
  created_at_ms  INTEGER NOT NULL,
  PRIMARY KEY(queue_id, referrer_kind, referrer_id)
) WITHOUT ROWID, STRICT;

CREATE TRIGGER queues_identity_update_guard
BEFORE UPDATE ON queues
WHEN OLD.id != NEW.id OR OLD.account_id != NEW.account_id OR
     OLD.lifecycle_generation != NEW.lifecycle_generation OR
     OLD.created_at_ms != NEW.created_at_ms OR
     (OLD.state = 'tombstoned' AND (
       NEW.state != OLD.state OR NEW.name != OLD.name OR
       NEW.config_generation != OLD.config_generation OR
       NEW.delivery_delay_seconds != OLD.delivery_delay_seconds OR
       NEW.retention_seconds != OLD.retention_seconds OR
       NEW.max_message_bytes != OLD.max_message_bytes OR
       NEW.max_batch_messages != OLD.max_batch_messages OR
       NEW.max_batch_bytes != OLD.max_batch_bytes OR
       NEW.max_backlog_bytes != OLD.max_backlog_bytes OR
       NEW.deleted_at_ms != OLD.deleted_at_ms
     ))
BEGIN
  SELECT RAISE(ABORT, 'queue immutable identity invariant');
END;

CREATE TRIGGER queues_transition_guard
BEFORE UPDATE OF state ON queues
WHEN OLD.state != NEW.state AND NOT (
  (OLD.state = 'creating' AND NEW.state IN ('ready', 'deleting')) OR
  (OLD.state = 'ready' AND NEW.state = 'deleting') OR
  (OLD.state = 'deleting' AND NEW.state = 'tombstoned')
)
BEGIN
  SELECT RAISE(ABORT, 'queue lifecycle transition invariant');
END;

CREATE TRIGGER queues_config_update_guard
BEFORE UPDATE ON queues
WHEN (
  OLD.delivery_delay_seconds != NEW.delivery_delay_seconds OR
  OLD.retention_seconds != NEW.retention_seconds OR
  OLD.max_message_bytes != NEW.max_message_bytes OR
  OLD.max_batch_messages != NEW.max_batch_messages OR
  OLD.max_batch_bytes != NEW.max_batch_bytes OR
  OLD.max_backlog_bytes != NEW.max_backlog_bytes
) AND (
  OLD.state != 'ready' OR NEW.state != 'ready' OR
  NEW.config_generation != OLD.config_generation + 1 OR
  NEW.availability != 'degraded' OR
  NEW.availability_code != 'QUEUE_CONFIG_PENDING'
)
BEGIN
  SELECT RAISE(ABORT, 'queue config generation invariant');
END;

CREATE TRIGGER queues_config_generation_guard
BEFORE UPDATE ON queues
WHEN OLD.config_generation != NEW.config_generation AND NOT (
  OLD.state = 'ready' AND NEW.state = 'ready' AND
  NEW.config_generation = OLD.config_generation + 1 AND
  (OLD.delivery_delay_seconds != NEW.delivery_delay_seconds OR
   OLD.retention_seconds != NEW.retention_seconds OR
   OLD.max_message_bytes != NEW.max_message_bytes OR
   OLD.max_batch_messages != NEW.max_batch_messages OR
   OLD.max_batch_bytes != NEW.max_batch_bytes OR
   OLD.max_backlog_bytes != NEW.max_backlog_bytes) AND
  NEW.availability = 'degraded' AND
  NEW.availability_code = 'QUEUE_CONFIG_PENDING'
)
BEGIN
  SELECT RAISE(ABORT, 'queue config generation invariant');
END;

CREATE TRIGGER queues_rename_guard
BEFORE UPDATE OF name ON queues
WHEN OLD.name != NEW.name AND NOT (
  OLD.state = 'ready' AND NEW.state = 'ready' AND
  OLD.availability = 'healthy' AND NEW.availability = 'healthy' AND
  OLD.config_generation = NEW.config_generation
)
BEGIN
  SELECT RAISE(ABORT, 'queue rename lifecycle invariant');
END;

CREATE TRIGGER queues_delete_referrer_guard
BEFORE UPDATE OF state ON queues
WHEN NEW.state = 'deleting' AND EXISTS (
  SELECT 1 FROM queue_referrers WHERE queue_id = OLD.id
)
BEGIN
  SELECT RAISE(ABORT, 'queue is referenced');
END;

CREATE TRIGGER queue_producer_bindings_insert_guard
BEFORE INSERT ON queue_producer_bindings
BEGIN
  SELECT CASE WHEN NEW.capability_version != 1
    THEN RAISE(ABORT, 'queue capability unsupported') END;
  SELECT CASE WHEN NEW.name GLOB '*[^A-Za-z0-9_]*' OR
                   NEW.name GLOB '[0-9]*' OR NEW.name GLOB 'OPEN_COMPUTE_*' OR
                   NEW.name GLOB '__*'
    THEN RAISE(ABORT, 'queue binding name invalid') END;
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM worker_versions d
    JOIN workers w ON w.id = d.worker_id
    JOIN queues q ON q.id = NEW.queue_id
    WHERE d.id = NEW.version_id AND d.state = 'staging'
      AND w.account_id = q.account_id
      AND q.state = 'ready' AND q.availability = 'healthy'
      AND q.lifecycle_generation = NEW.queue_lifecycle_generation
  ) THEN RAISE(ABORT, 'queue binding authority invariant') END;
  SELECT CASE WHEN EXISTS (
    SELECT 1 FROM version_vars v
      WHERE v.version_id = NEW.version_id AND v.name = NEW.name
    UNION ALL
    SELECT 1 FROM version_secrets s
      WHERE s.version_id = NEW.version_id AND s.name = NEW.name
    UNION ALL
    SELECT 1 FROM version_bindings b
      WHERE b.version_id = NEW.version_id AND b.name = NEW.name
  ) THEN RAISE(ABORT, 'queue binding env conflict') END;
END;

CREATE TRIGGER version_bindings_queue_name_guard
BEFORE INSERT ON version_bindings
WHEN EXISTS (
  SELECT 1 FROM queue_producer_bindings q
  WHERE q.version_id = NEW.version_id AND q.name = NEW.name
)
BEGIN
  SELECT RAISE(ABORT, 'version binding env conflict');
END;

CREATE TRIGGER version_vars_queue_name_guard
BEFORE INSERT ON version_vars
WHEN EXISTS (
  SELECT 1 FROM queue_producer_bindings q
  WHERE q.version_id = NEW.version_id AND q.name = NEW.name
)
BEGIN
  SELECT RAISE(ABORT, 'version variable Queue env conflict');
END;

CREATE TRIGGER version_secrets_queue_name_guard
BEFORE INSERT ON version_secrets
WHEN EXISTS (
  SELECT 1 FROM queue_producer_bindings q
  WHERE q.version_id = NEW.version_id AND q.name = NEW.name
)
BEGIN
  SELECT RAISE(ABORT, 'version secret Queue env conflict');
END;

CREATE TRIGGER queue_producer_bindings_update_guard
BEFORE UPDATE ON queue_producer_bindings
BEGIN
  SELECT RAISE(ABORT, 'queue producer binding is immutable');
END;

CREATE TRIGGER queue_producer_bindings_delete_guard
BEFORE DELETE ON queue_producer_bindings
WHEN NOT EXISTS (
  SELECT 1 FROM worker_versions d
  WHERE d.id = OLD.version_id AND d.state IN ('staging', 'deleting')
)
BEGIN
  SELECT RAISE(ABORT, 'queue producer binding delete invariant');
END;

CREATE TRIGGER queue_producer_bindings_referrer_insert
AFTER INSERT ON queue_producer_bindings
BEGIN
  INSERT INTO queue_referrers(queue_id, referrer_kind, referrer_id, created_at_ms)
  VALUES (NEW.queue_id, 'producer_binding', NEW.id, NEW.created_at_ms);
END;

CREATE TRIGGER queue_producer_bindings_referrer_delete
AFTER DELETE ON queue_producer_bindings
BEGIN
  DELETE FROM queue_referrers
  WHERE queue_id = OLD.queue_id AND referrer_kind = 'producer_binding'
    AND referrer_id = OLD.id;
END;

CREATE TRIGGER queue_referrers_producer_insert_guard
BEFORE INSERT ON queue_referrers
WHEN NEW.referrer_kind = 'producer_binding' AND NOT EXISTS (
  SELECT 1 FROM queue_producer_bindings b
  WHERE b.id = NEW.referrer_id AND b.queue_id = NEW.queue_id
)
BEGIN
  SELECT RAISE(ABORT, 'orphan queue producer referrer');
END;

CREATE TRIGGER queue_referrers_producer_delete_guard
BEFORE DELETE ON queue_referrers
WHEN OLD.referrer_kind = 'producer_binding' AND EXISTS (
  SELECT 1 FROM queue_producer_bindings b
  JOIN worker_versions d ON d.id = b.version_id
  JOIN workers w ON w.id = d.worker_id
  WHERE b.id = OLD.referrer_id AND b.queue_id = OLD.queue_id
    AND w.deleted_at_ms IS NULL
)
BEGIN
  SELECT RAISE(ABORT, 'live queue producer referrer');
END;
