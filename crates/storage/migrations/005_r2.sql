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

CREATE TABLE r2_objects (
  resource_id TEXT NOT NULL REFERENCES r2_buckets(resource_id) ON DELETE CASCADE,
  object_key TEXT NOT NULL,
  account_id TEXT NOT NULL,
  object_version TEXT NOT NULL,
  ssec_key_md5 TEXT,
  ssec_envelope TEXT,
  updated_at_ms INTEGER NOT NULL,
  PRIMARY KEY (resource_id, object_key),
  CHECK((ssec_key_md5 IS NULL) = (ssec_envelope IS NULL))
) STRICT;

CREATE TABLE r2_object_mutations (
  resource_id TEXT NOT NULL REFERENCES r2_buckets(resource_id) ON DELETE CASCADE,
  object_key TEXT NOT NULL,
  account_id TEXT NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN ('put', 'delete')),
  pending_version TEXT,
  pending_ssec_key_md5 TEXT,
  pending_ssec_envelope TEXT,
  started_at_ms INTEGER NOT NULL,
  PRIMARY KEY (resource_id, object_key),
  CHECK((pending_ssec_key_md5 IS NULL) = (pending_ssec_envelope IS NULL)),
  CHECK((kind = 'put') = (pending_version IS NOT NULL)),
  CHECK(kind = 'put' OR (pending_ssec_key_md5 IS NULL AND pending_ssec_envelope IS NULL))
) STRICT;

CREATE TABLE r2_multipart_uploads (
  upload_id TEXT PRIMARY KEY,
  resource_id TEXT NOT NULL REFERENCES r2_buckets(resource_id) ON DELETE CASCADE,
  account_id TEXT NOT NULL,
  object_key TEXT NOT NULL,
  provider_upload_id TEXT,
  storage_class TEXT NOT NULL CHECK(storage_class IN ('Standard', 'InfrequentAccess')),
  http_metadata TEXT NOT NULL,
  custom_metadata TEXT NOT NULL,
  ssec_key_md5 TEXT,
  ssec_envelope TEXT,
  object_version TEXT NOT NULL,
  completion_manifest TEXT,
  completed_metadata TEXT,
  state TEXT NOT NULL CHECK(state IN ('initiating', 'create_unknown', 'open', 'completing', 'completed', 'aborting', 'aborted')),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  CHECK((ssec_key_md5 IS NULL) = (ssec_envelope IS NULL)),
  CHECK(state IN ('initiating', 'create_unknown') OR provider_upload_id IS NOT NULL),
  CHECK((state IN ('completing', 'completed')) = (completion_manifest IS NOT NULL)),
  CHECK((state = 'completed') = (completed_metadata IS NOT NULL))
) STRICT;

CREATE INDEX r2_multipart_uploads_resource_state
  ON r2_multipart_uploads(resource_id, state);

CREATE TABLE r2_multipart_parts (
  upload_id TEXT NOT NULL REFERENCES r2_multipart_uploads(upload_id) ON DELETE CASCADE,
  part_number INTEGER NOT NULL CHECK(part_number >= 1 AND part_number <= 10000),
  etag TEXT NOT NULL,
  size INTEGER NOT NULL CHECK(size >= 0),
  uploaded_at_ms INTEGER NOT NULL,
  PRIMARY KEY (upload_id, part_number)
) STRICT;
