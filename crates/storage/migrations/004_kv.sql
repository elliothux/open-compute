CREATE TABLE kv_namespaces (
  resource_id          TEXT PRIMARY KEY REFERENCES resources(id),
  storage_key          TEXT NOT NULL UNIQUE,
  schema_version       INTEGER NOT NULL CHECK(schema_version >= 1),
  quota_bytes          INTEGER NOT NULL CHECK(quota_bytes >= 268435456),
  created_at_ms        INTEGER NOT NULL,
  last_opened_at_ms    INTEGER,
  last_quick_check_ms  INTEGER,
  last_backup_at_ms    INTEGER,
  restore_backup_id    TEXT REFERENCES kv_backups(id)
) STRICT;

CREATE TABLE kv_backups (
  id                    TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  source_resource_id    TEXT NOT NULL REFERENCES resources(id),
  state                 TEXT NOT NULL CHECK(state IN (
                          'creating', 'ready', 'failed', 'deleting', 'tombstoned'
                        )),
  object_key            TEXT,
  sha256                BLOB CHECK(sha256 IS NULL OR length(sha256) = 32),
  size_bytes            INTEGER CHECK(size_bytes IS NULL OR size_bytes >= 0),
  kv_schema_version     INTEGER NOT NULL CHECK(kv_schema_version >= 1),
  created_at_ms         INTEGER NOT NULL,
  completed_at_ms       INTEGER,
  error_code            TEXT,
  idempotency_key       TEXT NOT NULL CHECK(length(idempotency_key) BETWEEN 1 AND 128),
  request_fingerprint   BLOB NOT NULL CHECK(length(request_fingerprint) = 32),
  UNIQUE(source_resource_id, idempotency_key),
  CHECK((state = 'ready') =
        (object_key IS NOT NULL AND sha256 IS NOT NULL AND size_bytes IS NOT NULL))
) STRICT;

CREATE INDEX kv_backups_source
ON kv_backups(source_resource_id, created_at_ms, id);

CREATE TRIGGER kv_namespace_insert_guard
BEFORE INSERT ON kv_namespaces
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM resources
    WHERE id = NEW.resource_id
      AND kind = 'kv_namespace'
      AND state = 'creating'
      AND created_at_ms = NEW.created_at_ms
  ) THEN RAISE(ABORT, 'kv namespace authority invariant') END;
END;

CREATE TRIGGER kv_namespace_identity_immutable_guard
BEFORE UPDATE OF resource_id, storage_key, schema_version, quota_bytes, created_at_ms,
                 restore_backup_id
ON kv_namespaces
BEGIN
  SELECT RAISE(ABORT, 'immutable kv namespace identity');
END;

CREATE TRIGGER kv_namespace_delete_guard
BEFORE DELETE ON kv_namespaces
WHEN (SELECT state FROM resources WHERE id = OLD.resource_id)
  NOT IN ('deleting', 'tombstoned')
BEGIN
  SELECT RAISE(ABORT, 'live kv namespace locator');
END;

CREATE TRIGGER kv_resource_tombstone_retire_locator
AFTER UPDATE OF state ON resources
WHEN NEW.state = 'tombstoned' AND NEW.kind = 'kv_namespace'
BEGIN
  DELETE FROM kv_namespaces WHERE resource_id = NEW.id;
END;

CREATE TRIGGER kv_backup_insert_guard
BEFORE INSERT ON kv_backups
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM resources
    WHERE id = NEW.source_resource_id AND kind = 'kv_namespace'
  ) THEN RAISE(ABORT, 'kv backup source invariant') END;
END;

CREATE TRIGGER kv_backup_identity_immutable_guard
BEFORE UPDATE OF id, source_resource_id, kv_schema_version, created_at_ms,
                 idempotency_key, request_fingerprint
ON kv_backups
BEGIN
  SELECT RAISE(ABORT, 'immutable kv backup identity');
END;
