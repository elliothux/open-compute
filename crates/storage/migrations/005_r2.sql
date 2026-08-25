CREATE TABLE r2_buckets (
  resource_id           TEXT PRIMARY KEY REFERENCES resources(id),
  physical_prefix       TEXT NOT NULL UNIQUE,
  schema_version        INTEGER NOT NULL CHECK(schema_version >= 1),
  max_object_bytes      INTEGER NOT NULL CHECK(max_object_bytes > 0),
  provider_config_sha256 BLOB NOT NULL CHECK(length(provider_config_sha256) = 32),
  created_at_ms         INTEGER NOT NULL,
  delete_started_at_ms  INTEGER,
  last_probe_at_ms      INTEGER
) STRICT;

CREATE TRIGGER r2_bucket_insert_guard
BEFORE INSERT ON r2_buckets
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM resources
    WHERE id = NEW.resource_id
      AND kind = 'r2_bucket'
      AND state = 'creating'
      AND created_at_ms = NEW.created_at_ms
  ) THEN RAISE(ABORT, 'r2 bucket authority invariant') END;
END;

CREATE TRIGGER r2_bucket_identity_immutable_guard
BEFORE UPDATE OF resource_id, physical_prefix, schema_version,
                 max_object_bytes, provider_config_sha256, created_at_ms
ON r2_buckets
BEGIN
  SELECT RAISE(ABORT, 'immutable r2 bucket identity');
END;

CREATE TRIGGER r2_bucket_delete_guard
BEFORE DELETE ON r2_buckets
WHEN (SELECT state FROM resources WHERE id = OLD.resource_id) != 'tombstoned'
BEGIN
  SELECT RAISE(ABORT, 'live r2 bucket locator');
END;

CREATE TRIGGER r2_resource_tombstone_guard
BEFORE UPDATE OF state ON resources
WHEN NEW.state = 'tombstoned' AND NEW.kind = 'r2_bucket'
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM r2_buckets
    WHERE resource_id = NEW.id AND delete_started_at_ms IS NOT NULL
  ) THEN RAISE(ABORT, 'r2 bucket deletion not finalized') END;
END;

CREATE TRIGGER r2_resource_tombstone_retire_locator
AFTER UPDATE OF state ON resources
WHEN NEW.state = 'tombstoned' AND NEW.kind = 'r2_bucket'
BEGIN
  DELETE FROM r2_buckets WHERE resource_id = NEW.id;
END;
