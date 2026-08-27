CREATE TABLE deployment_queue_consumers (
  id                         TEXT PRIMARY KEY
                             CHECK(length(id) = 36 AND id = lower(id)),
  deployment_id              TEXT NOT NULL REFERENCES worker_deployments(id),
  queue_id                   TEXT NOT NULL REFERENCES queues(id),
  queue_lifecycle_generation INTEGER NOT NULL CHECK(queue_lifecycle_generation >= 1),
  entrypoint                 TEXT CHECK(entrypoint IS NULL OR length(entrypoint) BETWEEN 1 AND 128),
  max_batch_size             INTEGER NOT NULL CHECK(max_batch_size BETWEEN 1 AND 100),
  max_batch_timeout_seconds  INTEGER NOT NULL CHECK(max_batch_timeout_seconds BETWEEN 0 AND 60),
  max_retries                INTEGER NOT NULL CHECK(max_retries BETWEEN 0 AND 100),
  retry_delay_seconds        INTEGER NOT NULL CHECK(retry_delay_seconds BETWEEN 0 AND 86400),
  max_concurrency            INTEGER NOT NULL CHECK(max_concurrency BETWEEN 1 AND 4096),
  dlq_queue_id               TEXT REFERENCES queues(id),
  dlq_lifecycle_generation   INTEGER,
  capability_version         INTEGER NOT NULL CHECK(capability_version = 1),
  descriptor_sha256          BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms              INTEGER NOT NULL,
  UNIQUE(deployment_id, queue_id),
  CHECK((dlq_queue_id IS NULL) = (dlq_lifecycle_generation IS NULL)),
  CHECK(dlq_lifecycle_generation IS NULL OR dlq_lifecycle_generation >= 1),
  CHECK(dlq_queue_id IS NULL OR dlq_queue_id != queue_id)
) STRICT;

CREATE INDEX deployment_queue_consumers_queue
ON deployment_queue_consumers(queue_id, deployment_id, id);

CREATE TABLE queue_consumers (
  id                    TEXT PRIMARY KEY
                        CHECK(length(id) = 36 AND id = lower(id)),
  account_id            TEXT NOT NULL REFERENCES accounts(id),
  queue_id              TEXT NOT NULL REFERENCES queues(id),
  worker_id             TEXT NOT NULL REFERENCES workers(id),
  declaration_id        TEXT NOT NULL REFERENCES deployment_queue_consumers(id),
  deployment_id         TEXT NOT NULL REFERENCES worker_deployments(id),
  pending_declaration_id TEXT REFERENCES deployment_queue_consumers(id),
  pending_deployment_id TEXT REFERENCES worker_deployments(id),
  consumer_generation   INTEGER NOT NULL CHECK(consumer_generation >= 1),
  state                 TEXT NOT NULL CHECK(state IN (
                          'activating', 'active', 'paused', 'updating',
                          'deleting', 'tombstoned'
                        )),
  availability          TEXT NOT NULL CHECK(availability IN (
                          'healthy', 'degraded', 'unavailable'
                        )),
  availability_code     TEXT,
  created_at_ms         INTEGER NOT NULL,
  updated_at_ms         INTEGER NOT NULL,
  deleted_at_ms         INTEGER,
  CHECK(availability_code IS NULL OR length(availability_code) BETWEEN 1 AND 128),
  CHECK((pending_declaration_id IS NULL) = (pending_deployment_id IS NULL)),
  CHECK(pending_deployment_id IS NULL OR state IN ('updating', 'deleting')),
  CHECK((state = 'tombstoned') = (deleted_at_ms IS NOT NULL)),
  CHECK((availability = 'healthy') = (availability_code IS NULL))
) STRICT;

CREATE UNIQUE INDEX queue_one_live_consumer
ON queue_consumers(queue_id)
WHERE state != 'tombstoned';

CREATE INDEX queue_consumers_reconcile
ON queue_consumers(state, availability, updated_at_ms, id)
WHERE state IN ('activating', 'updating', 'deleting') OR availability != 'healthy';

CREATE TRIGGER deployment_queue_consumers_insert_guard
BEFORE INSERT ON deployment_queue_consumers
BEGIN
  SELECT CASE WHEN NEW.capability_version != 1
    THEN RAISE(ABORT, 'queue consumer capability unsupported') END;
  SELECT CASE WHEN NEW.entrypoint IS NOT NULL AND (
    NEW.entrypoint GLOB '*[^A-Za-z0-9_$]*' OR
    NEW.entrypoint GLOB '[0-9]*'
  ) THEN RAISE(ABORT, 'queue consumer entrypoint invalid') END;
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM worker_deployments d
    JOIN workers w ON w.id = d.worker_id
    JOIN queues q ON q.id = NEW.queue_id
    WHERE d.id = NEW.deployment_id AND d.state = 'staging'
      AND w.account_id = q.account_id
      AND q.state = 'ready' AND q.availability = 'healthy'
      AND q.lifecycle_generation = NEW.queue_lifecycle_generation
  ) THEN RAISE(ABORT, 'queue consumer authority invariant') END;
  SELECT CASE WHEN NEW.dlq_queue_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM worker_deployments d
    JOIN workers w ON w.id = d.worker_id
    JOIN queues q ON q.id = NEW.dlq_queue_id
    WHERE d.id = NEW.deployment_id
      AND w.account_id = q.account_id
      AND q.state = 'ready' AND q.availability = 'healthy'
      AND q.lifecycle_generation = NEW.dlq_lifecycle_generation
  ) THEN RAISE(ABORT, 'queue consumer DLQ authority invariant') END;
END;

CREATE TRIGGER deployment_queue_consumers_update_guard
BEFORE UPDATE ON deployment_queue_consumers
BEGIN
  SELECT RAISE(ABORT, 'queue consumer declaration is immutable');
END;

CREATE TRIGGER deployment_queue_consumers_delete_guard
BEFORE DELETE ON deployment_queue_consumers
WHEN NOT EXISTS (
  SELECT 1 FROM worker_deployments d
  WHERE d.id = OLD.deployment_id AND d.state IN ('staging', 'rejected', 'deleting')
)
BEGIN
  SELECT RAISE(ABORT, 'queue consumer declaration delete invariant');
END;

CREATE TRIGGER deployment_queue_consumers_referrers_insert
AFTER INSERT ON deployment_queue_consumers
BEGIN
  INSERT INTO queue_referrers(queue_id, referrer_kind, referrer_id, created_at_ms)
  VALUES (NEW.queue_id, 'consumer', NEW.id, NEW.created_at_ms);
  INSERT INTO queue_referrers(queue_id, referrer_kind, referrer_id, created_at_ms)
  SELECT NEW.dlq_queue_id, 'dlq', NEW.id, NEW.created_at_ms
  WHERE NEW.dlq_queue_id IS NOT NULL;
END;

CREATE TRIGGER deployment_queue_consumers_referrers_delete
AFTER DELETE ON deployment_queue_consumers
BEGIN
  DELETE FROM queue_referrers
  WHERE queue_id = OLD.queue_id AND referrer_kind = 'consumer' AND referrer_id = OLD.id;
  DELETE FROM queue_referrers
  WHERE queue_id = OLD.dlq_queue_id AND referrer_kind = 'dlq' AND referrer_id = OLD.id;
END;

CREATE TRIGGER queue_consumer_referrer_delete_guard
BEFORE DELETE ON queue_referrers
WHEN OLD.referrer_kind IN ('consumer', 'dlq') AND EXISTS (
  SELECT 1 FROM deployment_queue_consumers c
  WHERE c.id = OLD.referrer_id AND (
    (OLD.referrer_kind = 'consumer' AND c.queue_id = OLD.queue_id) OR
    (OLD.referrer_kind = 'dlq' AND c.dlq_queue_id = OLD.queue_id)
  )
)
BEGIN
  SELECT RAISE(ABORT, 'live queue consumer referrer');
END;

CREATE TRIGGER queue_consumers_insert_guard
BEFORE INSERT ON queue_consumers
BEGIN
  SELECT CASE WHEN NEW.state != 'activating' OR NEW.consumer_generation != 1 OR
                   NEW.availability != 'degraded' OR
                   NEW.availability_code != 'QUEUE_CONSUMER_PROJECTION_PENDING'
    THEN RAISE(ABORT, 'queue consumer activation invariant') END;
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM deployment_queue_consumers c
    JOIN worker_deployments d ON d.id = c.deployment_id
    JOIN workers w ON w.id = d.worker_id
    WHERE c.id = NEW.declaration_id AND c.deployment_id = NEW.deployment_id
      AND c.queue_id = NEW.queue_id AND d.state = 'ready'
      AND d.worker_id = NEW.worker_id AND w.account_id = NEW.account_id
  ) THEN RAISE(ABORT, 'queue consumer live authority invariant') END;
END;

CREATE TRIGGER queue_consumers_identity_guard
BEFORE UPDATE OF id, account_id, queue_id, worker_id, created_at_ms ON queue_consumers
BEGIN
  SELECT RAISE(ABORT, 'queue consumer identity is immutable');
END;

CREATE TRIGGER queue_consumers_transition_guard
BEFORE UPDATE OF state ON queue_consumers
WHEN OLD.state != NEW.state AND NOT (
  (OLD.state = 'activating' AND NEW.state IN ('active', 'paused', 'deleting')) OR
  (OLD.state = 'active' AND NEW.state IN ('paused', 'updating', 'deleting')) OR
  (OLD.state = 'paused' AND NEW.state IN ('active', 'updating', 'deleting')) OR
  (OLD.state = 'updating' AND NEW.state IN ('active', 'paused', 'deleting')) OR
  (OLD.state = 'deleting' AND NEW.state = 'tombstoned')
)
BEGIN
  SELECT RAISE(ABORT, 'queue consumer transition invariant');
END;

CREATE TRIGGER queue_consumers_generation_guard
BEFORE UPDATE ON queue_consumers
WHEN OLD.consumer_generation != NEW.consumer_generation AND NOT (
  NEW.consumer_generation = OLD.consumer_generation + 1 AND
  NEW.state = 'updating' AND NEW.availability = 'degraded' AND
  NEW.pending_declaration_id IS NOT NULL AND NEW.pending_deployment_id IS NOT NULL AND
  NEW.availability_code IN (
    'QUEUE_CONSUMER_DRAINING', 'QUEUE_CONSUMER_DRAINING_PAUSED'
  )
)
BEGIN
  SELECT RAISE(ABORT, 'queue consumer generation invariant');
END;

CREATE TRIGGER queue_consumers_pending_target_guard
BEFORE UPDATE OF pending_declaration_id, pending_deployment_id ON queue_consumers
WHEN NOT (
  (OLD.pending_declaration_id IS NULL AND OLD.pending_deployment_id IS NULL AND
   NEW.pending_declaration_id IS NOT NULL AND NEW.pending_deployment_id IS NOT NULL AND
   OLD.state IN ('active', 'paused') AND NEW.state = 'updating' AND
   NEW.consumer_generation = OLD.consumer_generation + 1 AND EXISTS (
     SELECT 1 FROM deployment_queue_consumers c
     JOIN worker_deployments d ON d.id = c.deployment_id
     WHERE c.id = NEW.pending_declaration_id
       AND c.deployment_id = NEW.pending_deployment_id
       AND c.queue_id = NEW.queue_id AND d.worker_id = NEW.worker_id
       AND d.state = 'ready'
   )) OR
  (OLD.pending_declaration_id IS NOT NULL AND OLD.pending_deployment_id IS NOT NULL AND
   NEW.pending_declaration_id IS NULL AND NEW.pending_deployment_id IS NULL AND
   OLD.state IN ('updating', 'deleting') AND NEW.state IN ('updating', 'tombstoned')) OR
  (OLD.pending_declaration_id IS NEW.pending_declaration_id AND
   OLD.pending_deployment_id IS NEW.pending_deployment_id)
)
BEGIN
  SELECT RAISE(ABORT, 'queue consumer pending target invariant');
END;

CREATE TRIGGER queue_consumers_target_guard
BEFORE UPDATE OF declaration_id, deployment_id ON queue_consumers
WHEN OLD.declaration_id != NEW.declaration_id OR OLD.deployment_id != NEW.deployment_id
BEGIN
  SELECT CASE WHEN OLD.state != 'updating' OR NEW.state != 'updating' OR
                   OLD.consumer_generation != NEW.consumer_generation OR
                   NEW.declaration_id != OLD.pending_declaration_id OR
                   NEW.deployment_id != OLD.pending_deployment_id OR
                   NEW.pending_declaration_id IS NOT NULL OR
                   NEW.pending_deployment_id IS NOT NULL OR NOT EXISTS (
    SELECT 1 FROM deployment_queue_consumers c
    JOIN worker_deployments d ON d.id = c.deployment_id
    WHERE c.id = NEW.declaration_id AND c.deployment_id = NEW.deployment_id
      AND c.queue_id = NEW.queue_id AND d.worker_id = NEW.worker_id AND d.state = 'ready'
  ) THEN RAISE(ABORT, 'queue consumer target invariant') END;
END;

CREATE TRIGGER queue_consumers_tombstone_guard
BEFORE UPDATE ON queue_consumers
WHEN OLD.state = 'tombstoned'
BEGIN
  SELECT RAISE(ABORT, 'queue consumer tombstone is immutable');
END;

CREATE TRIGGER queue_consumers_deployment_referrer_insert
AFTER INSERT ON queue_consumers
BEGIN
  INSERT INTO deployment_referrers(deployment_id, kind, ref_id, created_at_ms)
  VALUES (NEW.deployment_id, 'queue_consumer', NEW.id, NEW.created_at_ms);
END;

CREATE TRIGGER queue_consumers_deployment_referrer_update
AFTER UPDATE OF deployment_id ON queue_consumers
WHEN OLD.deployment_id != NEW.deployment_id
BEGIN
  DELETE FROM deployment_referrers
  WHERE deployment_id = OLD.deployment_id AND kind = 'queue_consumer' AND ref_id = OLD.id;
  INSERT INTO deployment_referrers(deployment_id, kind, ref_id, created_at_ms)
  VALUES (NEW.deployment_id, 'queue_consumer', NEW.id, NEW.updated_at_ms);
END;

CREATE TRIGGER queue_consumers_pending_referrer_insert
AFTER UPDATE OF pending_deployment_id ON queue_consumers
WHEN OLD.pending_deployment_id IS NULL AND NEW.pending_deployment_id IS NOT NULL
BEGIN
  INSERT INTO deployment_referrers(deployment_id, kind, ref_id, created_at_ms)
  VALUES (NEW.pending_deployment_id, 'queue_consumer_pending', NEW.id, NEW.updated_at_ms);
END;

CREATE TRIGGER queue_consumers_pending_referrer_delete
AFTER UPDATE OF pending_deployment_id ON queue_consumers
WHEN OLD.pending_deployment_id IS NOT NULL AND NEW.pending_deployment_id IS NULL
BEGIN
  DELETE FROM deployment_referrers
  WHERE deployment_id = OLD.pending_deployment_id
    AND kind = 'queue_consumer_pending' AND ref_id = OLD.id;
END;

CREATE TRIGGER queue_consumers_deployment_referrer_tombstone
AFTER UPDATE OF state ON queue_consumers
WHEN NEW.state = 'tombstoned'
BEGIN
  DELETE FROM deployment_referrers
  WHERE deployment_id = NEW.deployment_id AND kind = 'queue_consumer' AND ref_id = NEW.id;
END;
