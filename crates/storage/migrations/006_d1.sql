CREATE TABLE d1_backups (
  id                    TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  source_resource_id    TEXT NOT NULL REFERENCES resources(id),
  state                 TEXT NOT NULL CHECK(state IN (
                          'creating', 'ready', 'failed', 'deleting', 'tombstoned'
                        )),
  object_key            TEXT,
  sha256                BLOB CHECK(sha256 IS NULL OR length(sha256) = 32),
  size_bytes            INTEGER CHECK(size_bytes IS NULL OR size_bytes >= 0),
  d1_schema_version     INTEGER NOT NULL CHECK(d1_schema_version >= 1),
  sqlite_user_version   INTEGER NOT NULL CHECK(sqlite_user_version >= 0),
  created_at_ms         INTEGER NOT NULL,
  completed_at_ms       INTEGER,
  error_code            TEXT,
  idempotency_key       TEXT NOT NULL CHECK(length(idempotency_key) BETWEEN 1 AND 128),
  request_fingerprint   BLOB NOT NULL CHECK(length(request_fingerprint) = 32),
  UNIQUE(source_resource_id, idempotency_key),
  CHECK((state = 'ready') =
        (object_key IS NOT NULL AND sha256 IS NOT NULL AND size_bytes IS NOT NULL))
) STRICT;

CREATE INDEX d1_backups_source
ON d1_backups(source_resource_id, created_at_ms, id);

CREATE TABLE d1_databases (
  resource_id          TEXT PRIMARY KEY REFERENCES resources(id),
  storage_key          TEXT NOT NULL UNIQUE,
  schema_version       INTEGER NOT NULL CHECK(schema_version >= 1),
  quota_bytes          INTEGER NOT NULL CHECK(quota_bytes >= 67108864),
  created_at_ms        INTEGER NOT NULL,
  last_opened_at_ms    INTEGER,
  last_quick_check_ms  INTEGER,
  last_backup_at_ms    INTEGER,
  restore_backup_id    TEXT REFERENCES d1_backups(id)
) STRICT;

CREATE TRIGGER d1_database_insert_guard
BEFORE INSERT ON d1_databases
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM resources
    WHERE id = NEW.resource_id
      AND kind = 'd1_database'
      AND state = 'creating'
      AND created_at_ms = NEW.created_at_ms
  ) THEN RAISE(ABORT, 'd1 database authority invariant') END;
END;

CREATE TRIGGER d1_database_identity_immutable_guard
BEFORE UPDATE OF resource_id, storage_key, schema_version, quota_bytes, created_at_ms,
                 restore_backup_id
ON d1_databases
BEGIN
  SELECT RAISE(ABORT, 'immutable d1 database identity');
END;

CREATE TRIGGER d1_database_delete_guard
BEFORE DELETE ON d1_databases
WHEN (SELECT state FROM resources WHERE id = OLD.resource_id)
  NOT IN ('deleting', 'tombstoned')
BEGIN
  SELECT RAISE(ABORT, 'live d1 database locator');
END;

CREATE TRIGGER d1_resource_tombstone_retire_locator
AFTER UPDATE OF state ON resources
WHEN NEW.state = 'tombstoned' AND NEW.kind = 'd1_database'
BEGIN
  DELETE FROM d1_databases WHERE resource_id = NEW.id;
END;

CREATE TRIGGER d1_backup_insert_guard
BEFORE INSERT ON d1_backups
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM resources
    WHERE id = NEW.source_resource_id AND kind = 'd1_database'
  ) THEN RAISE(ABORT, 'd1 backup source invariant') END;
END;

CREATE TRIGGER d1_backup_identity_immutable_guard
BEFORE UPDATE OF id, source_resource_id, d1_schema_version, sqlite_user_version,
                 created_at_ms, idempotency_key, request_fingerprint
ON d1_backups
BEGIN
  SELECT RAISE(ABORT, 'immutable d1 backup identity');
END;
