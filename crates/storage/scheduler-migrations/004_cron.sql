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
