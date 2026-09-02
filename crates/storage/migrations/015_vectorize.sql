CREATE TABLE vectorize_indexes (
  resource_id     TEXT PRIMARY KEY REFERENCES resources(id),
  storage_key     TEXT NOT NULL UNIQUE,
  schema_version  INTEGER NOT NULL CHECK(schema_version = 1),
  dimensions      INTEGER NOT NULL CHECK(dimensions BETWEEN 32 AND 1536),
  metric          TEXT NOT NULL CHECK(metric IN ('cosine', 'euclidean', 'dot-product')),
  quota_vectors   INTEGER NOT NULL CHECK(quota_vectors BETWEEN 1 AND 200000),
  quota_bytes     INTEGER NOT NULL CHECK(quota_bytes >= 1048576),
  created_at_ms   INTEGER NOT NULL
) STRICT;

CREATE TRIGGER vectorize_index_insert_guard
BEFORE INSERT ON vectorize_indexes
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM resources
    WHERE id = NEW.resource_id
      AND kind = 'vectorize_index'
      AND state = 'creating'
      AND driver_schema_version = NEW.schema_version
      AND created_at_ms = NEW.created_at_ms
  ) THEN RAISE(ABORT, 'vectorize index authority invariant') END;
END;

CREATE TRIGGER vectorize_index_identity_immutable_guard
BEFORE UPDATE ON vectorize_indexes
BEGIN
  SELECT RAISE(ABORT, 'immutable vectorize index identity');
END;

CREATE TRIGGER vectorize_index_delete_guard
BEFORE DELETE ON vectorize_indexes
WHEN (SELECT state FROM resources WHERE id = OLD.resource_id)
  NOT IN ('deleting', 'tombstoned')
BEGIN
  SELECT RAISE(ABORT, 'live vectorize index locator');
END;

CREATE TRIGGER vectorize_resource_tombstone_retire_locator
AFTER UPDATE OF state ON resources
WHEN NEW.state = 'tombstoned' AND NEW.kind = 'vectorize_index'
BEGIN
  DELETE FROM vectorize_indexes WHERE resource_id = NEW.id;
END;
