-- Keep repair state only for unverified generations and known alarm projections.
CREATE TABLE do_alarm_repair (
  namespace_resource_id TEXT NOT NULL,
  object_id             TEXT NOT NULL,
  generation            INTEGER NOT NULL,
  revision              TEXT NOT NULL,
  expected_row_token    TEXT,
  expected_due_at_ms    INTEGER,
  expected_retry_count  INTEGER,
  PRIMARY KEY(namespace_resource_id, object_id, generation),
  FOREIGN KEY(namespace_resource_id, object_id, generation)
    REFERENCES do_objects(namespace_resource_id, object_id, generation) ON DELETE CASCADE,
  CHECK ((expected_row_token IS NULL) = (expected_due_at_ms IS NULL)
     AND (expected_row_token IS NULL) = (expected_retry_count IS NULL))
) STRICT, WITHOUT ROWID;

INSERT INTO do_alarm_repair (namespace_resource_id, object_id, generation, revision)
SELECT namespace_resource_id, object_id, generation,
       lower(hex(randomblob(4)) || '-' || hex(randomblob(2)) || '-4' ||
             substr(hex(randomblob(2)), 2) || '-' ||
             substr('89ab', 1 + (random() & 3), 1) ||
             substr(hex(randomblob(2)), 2) || '-' || hex(randomblob(6)))
FROM do_objects
WHERE state IN ('creating', 'ready');
