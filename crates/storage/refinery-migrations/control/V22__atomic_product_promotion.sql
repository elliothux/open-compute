-- Queue/Cron promotion now commits on the control connection. Claims keep their own version.

PRAGMA defer_foreign_keys=ON;

DROP TRIGGER queue_consumers_transition_guard;

DROP TRIGGER queue_consumers_generation_guard;

DROP TRIGGER queue_consumers_pending_target_guard;

DROP TRIGGER queue_consumers_target_guard;

DROP TRIGGER queue_consumers_tombstone_guard;

DROP TRIGGER queue_consumers_version_referrer_insert;

DROP TRIGGER queue_consumers_version_referrer_update;

DROP TRIGGER queue_consumers_pending_referrer_insert;

DROP TRIGGER queue_consumers_pending_referrer_delete;

DROP TRIGGER queue_consumers_version_referrer_tombstone;

DROP TRIGGER queue_consumers_api_referrers_target_switch;

DROP TRIGGER queue_consumers_api_referrers_tombstone;

DROP TRIGGER queue_consumers_insert_guard;

DROP TRIGGER queue_consumers_identity_guard;

DROP TRIGGER cron_activations_insert_guard;

DROP TRIGGER cron_activations_identity_guard;

DROP TRIGGER cron_activations_target_guard;

DROP TRIGGER cron_activations_transition_guard;

DROP TRIGGER cron_activations_tombstone_guard;

DROP TRIGGER cron_activations_referrer_insert;

DROP TRIGGER cron_activations_referrer_tombstone;

DROP TRIGGER queue_consumer_state_identity_guard;

DROP TRIGGER queue_consumer_state_generation_guard;

DROP TRIGGER queue_consumer_state_digest_guard;

DROP TRIGGER queue_delivery_batches_insert_guard;

DROP TRIGGER queue_delivery_batches_update_guard;

DROP TRIGGER queue_delivery_batches_delete_guard;

DROP TRIGGER queue_messages_transition_guard;

DROP TRIGGER queue_messages_claim_shape_delete_guard;

DROP TRIGGER cron_schedules_generation_digest_guard;

DROP TRIGGER cron_schedules_next_fire_guard;

DROP TRIGGER cron_runs_insert_guard;

DROP TRIGGER cron_runs_identity_guard;

DROP TRIGGER cron_runs_transition_guard;

DROP TRIGGER cron_schedules_identity_guard;

CREATE TABLE queue_consumers_next (
  id                    TEXT PRIMARY KEY
                        CHECK(length(id) = 36 AND id = lower(id)),
  queue_id              TEXT NOT NULL REFERENCES queues(id),
  worker_id             TEXT NOT NULL REFERENCES workers(id),
  declaration_id        TEXT NOT NULL REFERENCES version_queue_consumers(id),
  version_id         TEXT NOT NULL REFERENCES worker_versions(id),
  consumer_generation   INTEGER NOT NULL CHECK(consumer_generation >= 1),
  state                 TEXT NOT NULL CHECK(state IN (
                          'active', 'paused'
                        )),
  created_at_ms         INTEGER NOT NULL,
  updated_at_ms         INTEGER NOT NULL
) STRICT;

-- Recover the target of the committed deployment if an older handoff stopped halfway.
INSERT INTO queue_consumers_next
(id,queue_id,worker_id,declaration_id,version_id,consumer_generation,state,created_at_ms,updated_at_ms)
SELECT c.id,c.queue_id,c.worker_id,
       CASE WHEN d.origin='api' AND c.version_id=p.version_id THEN c.declaration_id
         ELSE COALESCE(desired.id,c.declaration_id) END,
       COALESCE(desired.version_id,c.version_id),c.consumer_generation,
       CASE WHEN c.state='paused' OR c.availability_code='QUEUE_CONSUMER_DRAINING_PAUSED'
         OR q.delivery_paused THEN 'paused' ELSE 'active' END,
       c.created_at_ms,c.updated_at_ms
FROM queue_consumers c JOIN version_queue_consumers d ON d.id=c.declaration_id
JOIN workers w ON w.id=c.worker_id JOIN worker_deployments p ON p.id=w.active_deployment_id
JOIN queues q ON q.id=c.queue_id
LEFT JOIN version_queue_consumers desired ON desired.version_id=p.version_id
  AND desired.queue_id=c.queue_id AND desired.origin='version'
WHERE c.state NOT IN ('deleting','tombstoned')
  AND (desired.id IS NOT NULL OR (d.origin='api' AND c.version_id=p.version_id));

DROP TABLE queue_consumers;

ALTER TABLE queue_consumers_next RENAME TO queue_consumers;

CREATE TABLE queue_consumer_state_next (
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
                                   'active', 'paused'
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

INSERT INTO queue_consumer_state_next
(consumer_id,queue_id,consumer_generation,version_id,worker_id,execution_generation,entrypoint,state,
 max_batch_size,max_batch_timeout_ms,max_retries,retry_delay_seconds,max_concurrency,
 dlq_queue_id,dlq_queue_generation,descriptor_sha256,updated_at_ms)
SELECT c.id,c.queue_id,c.consumer_generation,c.version_id,c.worker_id,
       COALESCE((SELECT execution_generation FROM queue_consumer_state old
                 WHERE old.consumer_id=c.id AND old.version_id=c.version_id),w.route_generation),
       d.entrypoint,c.state,d.max_batch_size,d.max_batch_timeout_seconds*1000,d.max_retries,
       d.retry_delay_seconds,d.max_concurrency,d.dlq_queue_id,d.dlq_lifecycle_generation,
       d.descriptor_sha256,c.updated_at_ms
FROM queue_consumers c JOIN version_queue_consumers d ON d.id=c.declaration_id
JOIN workers w ON w.id=c.worker_id JOIN queue_state q ON q.queue_id=c.queue_id;

DROP TABLE queue_consumer_state;

ALTER TABLE queue_consumer_state_next RENAME TO queue_consumer_state;

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
  claim_token           BLOB NOT NULL CHECK(length(claim_token) = 32),
  state                 TEXT NOT NULL CHECK(state = 'claimed'),
  claimed_at_ms         INTEGER NOT NULL,
  claim_until_ms        INTEGER NOT NULL,
  message_count         INTEGER NOT NULL CHECK(message_count BETWEEN 1 AND 100),
  created_at_ms         INTEGER NOT NULL,
  CHECK(claim_until_ms > claimed_at_ms)
) STRICT;

INSERT INTO queue_delivery_batches_next (id, queue_id, consumer_id, consumer_generation, version_id, execution_generation, entrypoint, claim_token, state, claimed_at_ms, claim_until_ms, message_count, created_at_ms) SELECT id, queue_id, consumer_id, consumer_generation, version_id, execution_generation, entrypoint, claim_token, state, claimed_at_ms, claim_until_ms, message_count, created_at_ms FROM queue_delivery_batches;

DROP TABLE queue_delivery_batches;

ALTER TABLE queue_delivery_batches_next RENAME TO queue_delivery_batches;

CREATE TEMP TABLE old_cron_targets AS SELECT activation_id,worker_id FROM cron_schedules;

CREATE TABLE cron_activations_next (
  id TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  worker_id TEXT NOT NULL REFERENCES workers(id),
  version_id TEXT NOT NULL REFERENCES worker_versions(id),
  expression TEXT NOT NULL CHECK(length(expression) BETWEEN 1 AND 256),
  expression_sha256 BLOB NOT NULL CHECK(length(expression_sha256) = 32),
  parser_version INTEGER NOT NULL CHECK(parser_version >= 1),
  scheduled_handler INTEGER NOT NULL CHECK(scheduled_handler IN (0, 1)),
  workflow_bindings_json BLOB NOT NULL CHECK(length(workflow_bindings_json) BETWEEN 2 AND 16384),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL
) STRICT;

INSERT INTO cron_activations_next (id, worker_id, version_id, expression, expression_sha256, parser_version, scheduled_handler, workflow_bindings_json, created_at_ms, updated_at_ms) SELECT id, worker_id, version_id, expression, expression_sha256, parser_version, scheduled_handler, workflow_bindings_json, created_at_ms, updated_at_ms FROM cron_activations WHERE version_id=(SELECT d.version_id FROM workers w JOIN worker_deployments d
ON d.id=w.active_deployment_id WHERE w.id=cron_activations.worker_id)
AND activation_generation=(SELECT MAX(old.activation_generation) FROM cron_activations old
  WHERE old.worker_id=cron_activations.worker_id AND old.version_id=cron_activations.version_id
    AND old.expression=cron_activations.expression);

DROP TABLE cron_activations;

ALTER TABLE cron_activations_next RENAME TO cron_activations;

CREATE TABLE cron_schedules_next (
  activation_id          TEXT PRIMARY KEY
                         CHECK(length(activation_id) = 36 AND activation_id = lower(activation_id)),
  worker_id              TEXT NOT NULL
                         CHECK(length(worker_id) = 36 AND worker_id = lower(worker_id)),
  version_id          TEXT NOT NULL
                         CHECK(length(version_id) = 36 AND version_id = lower(version_id)),
  execution_generation   INTEGER NOT NULL CHECK(execution_generation >= 1),
  expression             TEXT NOT NULL CHECK(length(expression) BETWEEN 1 AND 256),
  expression_sha256      BLOB NOT NULL CHECK(length(expression_sha256) = 32),
  parser_version         INTEGER NOT NULL CHECK(parser_version >= 1),
  next_fire_at_ms        INTEGER NOT NULL CHECK(next_fire_at_ms >= 0),
  updated_at_ms          INTEGER NOT NULL
) STRICT;

INSERT INTO cron_schedules_next
(activation_id,worker_id,version_id,execution_generation,expression,expression_sha256,
 parser_version,next_fire_at_ms,updated_at_ms)
SELECT a.id,a.worker_id,a.version_id,COALESCE(old.execution_generation,w.route_generation),
       a.expression,a.expression_sha256,a.parser_version,
       COALESCE(old.next_fire_at_ms,(a.updated_at_ms/60000+1)*60000),a.updated_at_ms
FROM cron_activations a JOIN workers w ON w.id=a.worker_id
LEFT JOIN cron_schedules old ON old.activation_id=a.id;

DROP TABLE cron_schedules;

ALTER TABLE cron_schedules_next RENAME TO cron_schedules;

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
  claim_token            BLOB,
  claimed_at_ms          INTEGER,
  claim_until_ms         INTEGER,
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
    (state = 'ready' AND next_attempt_at_ms IS NOT NULL AND claim_token IS NULL AND
      claimed_at_ms IS NULL AND claim_until_ms IS NULL AND completed_at_ms IS NULL) OR
    (state = 'claimed' AND next_attempt_at_ms IS NULL AND length(claim_token) = 32 AND
      claimed_at_ms IS NOT NULL AND claim_until_ms > claimed_at_ms AND
      claim_until_ms <= dispatch_deadline_at_ms AND completed_at_ms IS NULL) OR
    (state IN ('complete', 'failed', 'skipped') AND next_attempt_at_ms IS NULL AND
      claim_token IS NULL AND claimed_at_ms IS NULL AND claim_until_ms IS NULL AND
      completed_at_ms IS NOT NULL)
  )
) STRICT;

INSERT INTO cron_runs_next (id, activation_id, worker_id, scheduled_at_ms, version_id, execution_generation, expression, state, attempt, no_retry, next_attempt_at_ms, claim_token, claimed_at_ms, claim_until_ms, first_dispatched_at_ms, dispatch_deadline_at_ms, last_unknown_reason, error_code, created_at_ms, completed_at_ms) SELECT id, activation_id, (SELECT s.worker_id FROM old_cron_targets s WHERE s.activation_id=cron_runs.activation_id), scheduled_at_ms, version_id, execution_generation, expression, state, attempt, no_retry, next_attempt_at_ms, claim_token, claimed_at_ms, claim_until_ms, first_dispatched_at_ms, dispatch_deadline_at_ms, last_unknown_reason, error_code, created_at_ms, completed_at_ms FROM cron_runs;

DROP TABLE cron_runs;

ALTER TABLE cron_runs_next RENAME TO cron_runs;

DROP TABLE old_cron_targets;

CREATE INDEX queue_delivery_batches_expired
ON queue_delivery_batches(claim_until_ms, id);

CREATE INDEX queue_delivery_batches_consumer
ON queue_delivery_batches(consumer_id, consumer_generation, id);

CREATE INDEX cron_runs_due
ON cron_runs(state, next_attempt_at_ms, scheduled_at_ms, id)
WHERE state = 'ready';

CREATE INDEX cron_runs_expired
ON cron_runs(claim_until_ms, id)
WHERE state = 'claimed';

CREATE UNIQUE INDEX queue_one_live_consumer ON queue_consumers(queue_id);

CREATE INDEX cron_schedules_due ON cron_schedules(next_fire_at_ms, activation_id);

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
    OLD.attempt = NEW.attempt AND OLD.claim_token = NEW.claim_token) OR
  (OLD.state = NEW.state AND OLD.state IN ('complete', 'failed', 'skipped') AND
    OLD.id = NEW.id)
)
BEGIN
  SELECT RAISE(ABORT, 'cron run transition invariant');
END;

DELETE FROM version_referrers WHERE kind IN ('queue_consumer','queue_consumer_pending','cron_activation');

INSERT INTO version_referrers SELECT version_id,'queue_consumer',id,created_at_ms FROM queue_consumers;

INSERT INTO version_referrers SELECT version_id,'cron_activation',id,created_at_ms FROM cron_activations;

CREATE TRIGGER queue_consumers_pin_insert AFTER INSERT ON queue_consumers WHEN NEW.version_id IN (SELECT id FROM worker_versions) BEGIN INSERT INTO version_referrers VALUES(NEW.version_id,'queue_consumer',NEW.id,NEW.created_at_ms); END;

CREATE TRIGGER queue_consumers_pin_delete AFTER DELETE ON queue_consumers BEGIN DELETE FROM version_referrers WHERE version_id=OLD.version_id AND kind='queue_consumer' AND ref_id=OLD.id; END;

CREATE TRIGGER cron_activations_pin_insert AFTER INSERT ON cron_activations WHEN NEW.version_id IN (SELECT id FROM worker_versions) BEGIN INSERT INTO version_referrers VALUES(NEW.version_id,'cron_activation',NEW.id,NEW.created_at_ms); END;

CREATE TRIGGER cron_activations_pin_delete AFTER DELETE ON cron_activations BEGIN DELETE FROM version_referrers WHERE version_id=OLD.version_id AND kind='cron_activation' AND ref_id=OLD.id; END;

INSERT INTO version_referrers SELECT version_id,'queue_batch',id,created_at_ms FROM queue_delivery_batches WHERE version_id IN (SELECT id FROM worker_versions);

CREATE TRIGGER queue_delivery_batches_pin_insert AFTER INSERT ON queue_delivery_batches WHEN NEW.version_id IN (SELECT id FROM worker_versions) BEGIN INSERT INTO version_referrers VALUES(NEW.version_id,'queue_batch',NEW.id,NEW.created_at_ms); END;

CREATE TRIGGER queue_delivery_batches_pin_delete AFTER DELETE ON queue_delivery_batches BEGIN DELETE FROM version_referrers WHERE version_id=OLD.version_id AND kind='queue_batch' AND ref_id=OLD.id; END;

INSERT INTO version_referrers SELECT version_id,'cron_run',id,created_at_ms FROM cron_runs WHERE version_id IN (SELECT id FROM worker_versions);

CREATE TRIGGER cron_runs_pin_insert AFTER INSERT ON cron_runs WHEN NEW.version_id IN (SELECT id FROM worker_versions) BEGIN INSERT INTO version_referrers VALUES(NEW.version_id,'cron_run',NEW.id,NEW.created_at_ms); END;

CREATE TRIGGER cron_runs_pin_delete AFTER DELETE ON cron_runs BEGIN DELETE FROM version_referrers WHERE version_id=OLD.version_id AND kind='cron_run' AND ref_id=OLD.id; END;

CREATE TRIGGER queue_consumers_pin_update AFTER UPDATE OF version_id ON queue_consumers WHEN OLD.version_id!=NEW.version_id BEGIN
DELETE FROM version_referrers WHERE version_id=OLD.version_id AND kind='queue_consumer' AND ref_id=OLD.id;
INSERT INTO version_referrers VALUES(NEW.version_id,'queue_consumer',NEW.id,NEW.updated_at_ms); END;

CREATE TRIGGER queue_consumers_api_referrers_delete AFTER DELETE ON queue_consumers BEGIN
DELETE FROM queue_referrers WHERE referrer_id=OLD.declaration_id AND referrer_kind IN ('consumer','dlq') AND EXISTS(SELECT 1 FROM version_queue_consumers WHERE id=OLD.declaration_id AND origin='api'); END;

CREATE TRIGGER queue_consumers_api_referrers_update AFTER UPDATE OF declaration_id ON queue_consumers WHEN OLD.declaration_id!=NEW.declaration_id BEGIN
DELETE FROM queue_referrers WHERE referrer_id=OLD.declaration_id AND referrer_kind IN ('consumer','dlq') AND EXISTS(SELECT 1 FROM version_queue_consumers WHERE id=OLD.declaration_id AND origin='api'); END;

-- API declarations no longer referenced by an attachment must not block Queue deletion.
DELETE FROM queue_referrers WHERE referrer_kind IN ('consumer','dlq')
  AND referrer_id IN (SELECT id FROM version_queue_consumers WHERE origin='api')
  AND referrer_id NOT IN (SELECT declaration_id FROM queue_consumers);

CREATE TRIGGER cron_runs_pin_terminal AFTER UPDATE OF state ON cron_runs
WHEN NEW.state IN ('complete','failed','skipped') BEGIN
  DELETE FROM version_referrers WHERE kind='cron_run' AND ref_id=NEW.id;
END;
DELETE FROM version_referrers WHERE kind='cron_run' AND ref_id IN
  (SELECT id FROM cron_runs WHERE state IN ('complete','failed','skipped'));
