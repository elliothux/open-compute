-- Retention and quota eviction order by arrival time. The previous indexes
-- cover event time, script, and version, so every insert scanned the table.
CREATE INDEX observability_invocations_received
ON observability_invocations(received_at_ms, invocation_id);

-- Recompute the counter once. Later deletes subtract RETURNING byte_size.
UPDATE observability_maintenance
SET accounted_bytes = (
  SELECT COALESCE(SUM(byte_size), 0) FROM observability_invocations
)
WHERE singleton = 1;
