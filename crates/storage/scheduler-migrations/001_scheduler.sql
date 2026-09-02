CREATE TABLE scheduler_meta (
  singleton          INTEGER PRIMARY KEY CHECK(singleton = 1),
  schema_version     INTEGER NOT NULL,
  data_format        TEXT NOT NULL,
  created_at_ms      INTEGER NOT NULL,
  updated_at_ms      INTEGER NOT NULL
) STRICT;

CREATE TABLE scheduler_migrations (
  version         INTEGER PRIMARY KEY,
  name            TEXT NOT NULL,
  checksum_sha256 BLOB NOT NULL CHECK(length(checksum_sha256) = 32),
  applied_at_ms   INTEGER NOT NULL,
  app_version     TEXT NOT NULL
) STRICT;

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
