CREATE TABLE observability_meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
) WITHOUT ROWID, STRICT;

INSERT INTO observability_meta(key, value)
VALUES ('data_format', 'open-compute-observability-v1');

CREATE TABLE observability_invocations (
  invocation_id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL,
  script_name TEXT NOT NULL,
  version_id TEXT NOT NULL,
  deployment_id TEXT,
  event_timestamp_ms INTEGER NOT NULL,
  received_at_ms INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  outcome TEXT NOT NULL,
  cpu_time_ms REAL NOT NULL,
  wall_time_ms REAL NOT NULL,
  truncated INTEGER NOT NULL CHECK(truncated IN (0, 1)),
  event_json BLOB NOT NULL,
  byte_size INTEGER NOT NULL CHECK(byte_size > 0)
) STRICT;

CREATE INDEX observability_invocations_account_time
ON observability_invocations(account_id, event_timestamp_ms DESC, invocation_id DESC);

CREATE INDEX observability_invocations_script_time
ON observability_invocations(account_id, script_name, event_timestamp_ms DESC, invocation_id DESC);

CREATE INDEX observability_invocations_version_time
ON observability_invocations(account_id, version_id, event_timestamp_ms DESC, invocation_id DESC);

CREATE TABLE observability_events (
  event_id TEXT PRIMARY KEY,
  invocation_id TEXT NOT NULL REFERENCES observability_invocations(invocation_id) ON DELETE CASCADE,
  account_id TEXT NOT NULL,
  script_name TEXT NOT NULL,
  version_id TEXT NOT NULL,
  timestamp_ms INTEGER NOT NULL,
  sequence INTEGER NOT NULL CHECK(sequence >= 0),
  metadata_type TEXT NOT NULL CHECK(metadata_type IN ('cf-worker-event', 'cf-worker-log')),
  level TEXT,
  source_json BLOB NOT NULL,
  metadata_json BLOB NOT NULL,
  byte_size INTEGER NOT NULL CHECK(byte_size > 0),
  UNIQUE(invocation_id, sequence)
) STRICT;

CREATE INDEX observability_events_account_time
ON observability_events(account_id, timestamp_ms DESC, event_id DESC);

CREATE INDEX observability_events_script_time
ON observability_events(account_id, script_name, timestamp_ms DESC, event_id DESC);

CREATE INDEX observability_events_invocation
ON observability_events(account_id, invocation_id, sequence);

CREATE TABLE observability_fields (
  event_id TEXT NOT NULL REFERENCES observability_events(event_id) ON DELETE CASCADE,
  key TEXT NOT NULL,
  value_type TEXT NOT NULL CHECK(value_type IN ('string', 'number', 'boolean')),
  string_value TEXT,
  number_value REAL,
  boolean_value INTEGER CHECK(boolean_value IS NULL OR boolean_value IN (0, 1)),
  PRIMARY KEY(event_id, key),
  CHECK(
    (value_type = 'string' AND string_value IS NOT NULL AND number_value IS NULL AND boolean_value IS NULL) OR
    (value_type = 'number' AND string_value IS NULL AND number_value IS NOT NULL AND boolean_value IS NULL) OR
    (value_type = 'boolean' AND string_value IS NULL AND number_value IS NULL AND boolean_value IS NOT NULL)
  )
) WITHOUT ROWID, STRICT;

CREATE INDEX observability_fields_key_type
ON observability_fields(key, value_type, event_id);

CREATE TABLE observability_maintenance (
  singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
  accounted_bytes INTEGER NOT NULL CHECK(accounted_bytes >= 0),
  last_gc_at_ms INTEGER
) STRICT;

INSERT INTO observability_maintenance(singleton, accounted_bytes, last_gc_at_ms)
VALUES (1, 0, NULL);
