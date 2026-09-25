-- The database is instance-local. Reject mixed or malformed historical rows
-- before removing their redundant account columns.
CREATE TABLE observability_identity (
  singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
  instance_id TEXT NOT NULL
    CHECK(length(instance_id) = 32 AND instance_id = lower(instance_id)
      AND instance_id NOT GLOB '*[^0-9a-f]*')
) STRICT;

WITH owners AS (
  SELECT account_id FROM observability_invocations
  UNION SELECT account_id FROM observability_events
)
INSERT INTO observability_identity(singleton, instance_id)
SELECT 1, replace(account_id, '-', '') FROM owners;

DROP INDEX observability_invocations_account_time;
DROP INDEX observability_invocations_script_time;
DROP INDEX observability_invocations_version_time;
DROP INDEX observability_events_account_time;
DROP INDEX observability_events_script_time;
DROP INDEX observability_events_invocation;

ALTER TABLE observability_invocations DROP COLUMN account_id;
ALTER TABLE observability_events DROP COLUMN account_id;

CREATE INDEX observability_invocations_time
ON observability_invocations(event_timestamp_ms DESC, invocation_id DESC);
CREATE INDEX observability_invocations_script_time
ON observability_invocations(script_name, event_timestamp_ms DESC, invocation_id DESC);
CREATE INDEX observability_invocations_version_time
ON observability_invocations(version_id, event_timestamp_ms DESC, invocation_id DESC);
CREATE INDEX observability_events_time
ON observability_events(timestamp_ms DESC, event_id DESC);
CREATE INDEX observability_events_script_time
ON observability_events(script_name, timestamp_ms DESC, event_id DESC);
CREATE INDEX observability_events_invocation
ON observability_events(invocation_id, sequence);
