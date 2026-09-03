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

-- Only fully durable, independently recoverable database states are indexed here.
-- Staging and failed snapshot attempts never become bookmark authority.
CREATE TABLE d1_snapshots (
  resource_id          TEXT NOT NULL REFERENCES d1_databases(resource_id) ON DELETE CASCADE,
  session_version      INTEGER NOT NULL CHECK(session_version >= 0),
  snapshot_key         TEXT NOT NULL UNIQUE CHECK(
                         length(snapshot_key) BETWEEN 1 AND 512
                         AND instr(snapshot_key, '..') = 0
                       ),
  sha256               BLOB NOT NULL CHECK(length(sha256) = 32),
  size_bytes           INTEGER NOT NULL CHECK(size_bytes > 0),
  created_at_ms        INTEGER NOT NULL CHECK(created_at_ms >= 0),
  PRIMARY KEY(resource_id, session_version)
) STRICT, WITHOUT ROWID;

CREATE INDEX d1_snapshots_timestamp
ON d1_snapshots(resource_id, created_at_ms DESC, session_version DESC);

CREATE TRIGGER d1_snapshot_immutable_guard
BEFORE UPDATE ON d1_snapshots
BEGIN
  SELECT RAISE(ABORT, 'immutable completed d1 snapshot');
END;

-- Persistent SQL transfer sessions back Cloudflare D1 export/import polling.
-- URL capabilities are never stored: only their keyed fingerprints are retained.
CREATE TABLE d1_transfer_sessions (
  id                    TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  resource_id           TEXT NOT NULL,
  kind                  TEXT NOT NULL CHECK(kind IN ('export', 'import')),
  state                 TEXT NOT NULL CHECK(state IN (
                          'preparing', 'uploading', 'uploaded', 'ingesting',
                          'complete', 'failed', 'expired'
                        )),
  at_session_version    INTEGER NOT NULL CHECK(at_session_version >= 0),
  result_session_version INTEGER CHECK(result_session_version >= 0),
  filename              TEXT NOT NULL CHECK(
                          length(filename) BETWEEN 1 AND 255
                          AND instr(filename, '/') = 0
                          AND instr(filename, char(0)) = 0
                          AND filename NOT IN ('.', '..')
                        ),
  file_key              TEXT CHECK(
                          file_key IS NULL OR (
                            length(file_key) BETWEEN 1 AND 512
                            AND instr(file_key, '..') = 0
                          )
                        ),
  etag_md5              BLOB CHECK(etag_md5 IS NULL OR length(etag_md5) = 16),
  sha256                BLOB CHECK(sha256 IS NULL OR length(sha256) = 32),
  size_bytes            INTEGER CHECK(size_bytes IS NULL OR size_bytes > 0),
  token_fingerprint     BLOB NOT NULL CHECK(length(token_fingerprint) = 32),
  token_action          TEXT NOT NULL CHECK(token_action IN ('upload', 'download')),
  token_expires_at_ms   INTEGER NOT NULL,
  num_queries           INTEGER CHECK(num_queries IS NULL OR num_queries >= 0),
  created_at_ms         INTEGER NOT NULL CHECK(created_at_ms >= 0),
  updated_at_ms         INTEGER NOT NULL CHECK(updated_at_ms >= created_at_ms),
  completed_at_ms       INTEGER,
  error_code            TEXT,
  FOREIGN KEY(resource_id)
    REFERENCES d1_databases(resource_id) ON DELETE CASCADE,
  FOREIGN KEY(resource_id, at_session_version)
    REFERENCES d1_snapshots(resource_id, session_version),
  FOREIGN KEY(resource_id, result_session_version)
    REFERENCES d1_snapshots(resource_id, session_version),
  UNIQUE(resource_id, kind, filename),
  CHECK(token_expires_at_ms > created_at_ms),
  CHECK((kind = 'export' AND token_action = 'download' AND etag_md5 IS NULL)
     OR (kind = 'import' AND token_action = 'upload' AND etag_md5 IS NOT NULL)),
  CHECK((file_key IS NULL AND sha256 IS NULL AND size_bytes IS NULL)
     OR (file_key IS NOT NULL AND sha256 IS NOT NULL AND size_bytes IS NOT NULL)),
  CHECK(state NOT IN ('preparing', 'uploading')
        OR (file_key IS NULL AND sha256 IS NULL AND size_bytes IS NULL)),
  CHECK(state NOT IN ('uploaded', 'ingesting', 'complete')
        OR (file_key IS NOT NULL AND sha256 IS NOT NULL AND size_bytes IS NOT NULL)),
  CHECK(kind != 'export' OR num_queries IS NULL),
  CHECK(state NOT IN ('preparing', 'uploading', 'uploaded') OR num_queries IS NULL),
  CHECK(state != 'ingesting' OR (kind = 'import' AND num_queries IS NOT NULL)),
  CHECK((state = 'complete' AND kind = 'import') =
        (result_session_version IS NOT NULL AND num_queries IS NOT NULL)),
  CHECK((state IN ('complete', 'failed', 'expired')) = (completed_at_ms IS NOT NULL)),
  CHECK((state = 'failed') = (error_code IS NOT NULL))
) STRICT;

CREATE INDEX d1_transfer_sessions_resource
ON d1_transfer_sessions(resource_id, created_at_ms DESC, id);

CREATE UNIQUE INDEX d1_transfer_sessions_active
ON d1_transfer_sessions(resource_id)
WHERE state IN ('preparing', 'uploading', 'uploaded', 'ingesting');

CREATE TRIGGER d1_transfer_identity_immutable_guard
BEFORE UPDATE OF id, resource_id, kind, at_session_version, filename, etag_md5,
                 token_fingerprint, token_action, token_expires_at_ms, created_at_ms
ON d1_transfer_sessions
BEGIN
  SELECT RAISE(ABORT, 'immutable d1 transfer identity');
END;

CREATE TRIGGER d1_transfer_transition_guard
BEFORE UPDATE OF state ON d1_transfer_sessions
BEGIN
  SELECT CASE WHEN NOT (
       (OLD.state = 'preparing' AND NEW.state IN ('complete', 'failed', 'expired'))
    OR (OLD.state = 'uploading' AND NEW.state IN ('uploaded', 'failed', 'expired'))
    OR (OLD.state = 'uploaded' AND NEW.state IN ('ingesting', 'failed', 'expired'))
    OR (OLD.state = 'ingesting' AND NEW.state IN ('complete', 'failed', 'expired'))
  ) THEN RAISE(ABORT, 'invalid d1 transfer transition') END;
  SELECT CASE WHEN NEW.updated_at_ms < OLD.updated_at_ms
    THEN RAISE(ABORT, 'd1 transfer time moved backwards') END;
END;

CREATE TRIGGER d1_transfer_file_evidence_immutable_guard
BEFORE UPDATE OF file_key, sha256, size_bytes ON d1_transfer_sessions
WHEN OLD.file_key IS NOT NULL AND (
     NEW.file_key IS NOT OLD.file_key
  OR NEW.sha256 IS NOT OLD.sha256
  OR NEW.size_bytes IS NOT OLD.size_bytes
)
BEGIN
  SELECT RAISE(ABORT, 'immutable d1 transfer file evidence');
END;

CREATE TRIGGER d1_transfer_result_guard
BEFORE UPDATE OF result_session_version, num_queries ON d1_transfer_sessions
WHEN NOT (
  (OLD.state = 'uploaded' AND NEW.state = 'ingesting'
   AND OLD.result_session_version IS NULL AND NEW.result_session_version IS NULL
   AND OLD.num_queries IS NULL AND NEW.num_queries IS NOT NULL)
  OR
  (OLD.state = 'ingesting' AND NEW.state = 'complete'
   AND OLD.result_session_version IS NULL AND NEW.result_session_version IS NOT NULL
   AND OLD.num_queries IS NEW.num_queries)
)
BEGIN
  SELECT RAISE(ABORT, 'invalid d1 transfer result');
END;

CREATE TRIGGER d1_transfer_terminal_immutable_guard
BEFORE UPDATE ON d1_transfer_sessions
WHEN OLD.state IN ('complete', 'failed', 'expired')
BEGIN
  SELECT RAISE(ABORT, 'immutable terminal d1 transfer');
END;

-- A restore changes the existing database identity-preservingly. One durable
-- intent fences each database until its replacement is either aborted before
-- publication or reconciled to a completed snapshot after publication.
CREATE TABLE d1_restore_intents (
  id                    TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  resource_id           TEXT NOT NULL UNIQUE,
  source_session_version INTEGER NOT NULL CHECK(source_session_version >= 0),
  previous_session_version INTEGER NOT NULL CHECK(previous_session_version >= 0),
  result_session_version INTEGER NOT NULL CHECK(
                           result_session_version = previous_session_version + 1
                         ),
  request_fingerprint   BLOB NOT NULL CHECK(length(request_fingerprint) = 32),
  created_at_ms         INTEGER NOT NULL CHECK(created_at_ms >= 0),
  FOREIGN KEY(resource_id)
    REFERENCES d1_databases(resource_id) ON DELETE CASCADE,
  FOREIGN KEY(resource_id, source_session_version)
    REFERENCES d1_snapshots(resource_id, session_version),
  FOREIGN KEY(resource_id, previous_session_version)
    REFERENCES d1_snapshots(resource_id, session_version)
) STRICT;

CREATE TRIGGER d1_restore_intent_immutable_guard
BEFORE UPDATE ON d1_restore_intents
BEGIN
  SELECT RAISE(ABORT, 'immutable d1 restore intent');
END;
