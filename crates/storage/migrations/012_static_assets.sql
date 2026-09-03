CREATE TABLE version_assets (
  version_id TEXT PRIMARY KEY REFERENCES worker_versions(id),
  manifest_sha256 BLOB NOT NULL CHECK(length(manifest_sha256) = 32),
  manifest_size INTEGER NOT NULL CHECK(manifest_size > 0),
  manifest_schema_version INTEGER NOT NULL CHECK(manifest_schema_version = 1),
  manifest_json BLOB NOT NULL,
  routing_config_json BLOB NOT NULL,
  binding_name TEXT,
  logical_file_count INTEGER NOT NULL CHECK(logical_file_count > 0),
  logical_total_bytes INTEGER NOT NULL CHECK(logical_total_bytes >= 0),
  created_at_ms INTEGER NOT NULL,
  CHECK(binding_name IS NULL OR length(binding_name) BETWEEN 1 AND 64)
) WITHOUT ROWID, STRICT;

CREATE TABLE version_object_refs (
  version_id TEXT NOT NULL REFERENCES worker_versions(id),
  object_kind TEXT NOT NULL CHECK(object_kind IN ('bundle', 'asset_manifest', 'asset_blob')),
  sha256 BLOB NOT NULL CHECK(length(sha256) = 32),
  size INTEGER NOT NULL CHECK(size >= 0),
  created_at_ms INTEGER NOT NULL,
  PRIMARY KEY(version_id, object_kind, sha256)
) WITHOUT ROWID, STRICT;

CREATE INDEX version_object_refs_digest
ON version_object_refs(sha256, version_id);

CREATE TABLE version_uploads (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  worker_id TEXT NOT NULL REFERENCES workers(id),
  idempotency_key TEXT NOT NULL,
  input_fingerprint BLOB NOT NULL CHECK(length(input_fingerprint) = 32),
  content_kind TEXT NOT NULL CHECK(content_kind IN ('worker', 'assets_only')),
  bundle_sha256 BLOB CHECK(bundle_sha256 IS NULL OR length(bundle_sha256) = 32),
  bundle_size INTEGER CHECK(bundle_size IS NULL OR bundle_size >= 0),
  manifest_sha256 BLOB NOT NULL CHECK(length(manifest_sha256) = 32),
  manifest_size INTEGER NOT NULL CHECK(manifest_size > 0),
  manifest_json BLOB NOT NULL,
  routing_config_json BLOB NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('open', 'finalizing', 'committed', 'aborted', 'expired')),
  version_id TEXT,
  finalize_fingerprint BLOB CHECK(finalize_fingerprint IS NULL OR length(finalize_fingerprint) = 32),
  finalize_owner_startup_id TEXT,
  finalize_response_json BLOB,
  finalize_error_code TEXT,
  created_at_ms INTEGER NOT NULL,
  expires_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  CHECK(
    (content_kind = 'worker' AND bundle_sha256 IS NOT NULL AND bundle_size IS NOT NULL) OR
    (content_kind = 'assets_only' AND bundle_sha256 IS NULL AND bundle_size IS NULL)
  ),
  CHECK(
    (status IN ('open', 'aborted', 'expired') AND version_id IS NULL
      AND finalize_fingerprint IS NULL AND finalize_owner_startup_id IS NULL
      AND finalize_response_json IS NULL AND finalize_error_code IS NULL) OR
    (status = 'finalizing' AND version_id IS NOT NULL
      AND finalize_fingerprint IS NOT NULL AND finalize_owner_startup_id IS NOT NULL
      AND finalize_response_json IS NULL AND finalize_error_code IS NULL) OR
    (status = 'committed' AND version_id IS NOT NULL
      AND finalize_fingerprint IS NOT NULL AND finalize_owner_startup_id IS NOT NULL
      AND ((finalize_response_json IS NOT NULL AND finalize_error_code IS NULL) OR
           (finalize_response_json IS NULL AND finalize_error_code IS NOT NULL)))
  ),
  UNIQUE(account_id, worker_id, idempotency_key)
) STRICT;

CREATE INDEX version_uploads_worker_status
ON version_uploads(account_id, worker_id, status, expires_at_ms);

CREATE TABLE asset_upload_sessions (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  script_name TEXT NOT NULL CHECK(length(script_name) BETWEEN 1 AND 63),
  status TEXT NOT NULL CHECK(status IN ('open', 'complete', 'reserved', 'consumed', 'expired')),
  reservation_id TEXT,
  released_reservation_id TEXT,
  created_at_ms INTEGER NOT NULL,
  expires_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  CHECK(
    (status IN ('open', 'expired') AND reservation_id IS NULL AND released_reservation_id IS NULL) OR
    (status = 'complete' AND reservation_id IS NULL) OR
    (status IN ('reserved', 'consumed') AND reservation_id IS NOT NULL AND released_reservation_id IS NULL)
  )
) STRICT;

CREATE TRIGGER asset_upload_session_identity_immutable
BEFORE UPDATE ON asset_upload_sessions
WHEN NEW.id != OLD.id OR NEW.account_id != OLD.account_id
  OR NEW.script_name != OLD.script_name OR NEW.created_at_ms != OLD.created_at_ms
  OR NEW.expires_at_ms != OLD.expires_at_ms
BEGIN
  SELECT RAISE(ABORT, 'asset upload session identity is immutable');
END;

CREATE TRIGGER asset_upload_session_transition_guard
BEFORE UPDATE ON asset_upload_sessions
WHEN NOT (
  (OLD.status = 'open' AND NEW.status IN ('open', 'complete', 'expired')) OR
  (OLD.status = 'complete' AND NEW.status IN ('complete', 'reserved')) OR
  (OLD.status = 'reserved' AND NEW.status IN ('reserved', 'complete', 'consumed')) OR
  (OLD.status = 'consumed' AND NEW.status = 'consumed') OR
  (OLD.status = 'expired' AND NEW.status = 'expired')
)
BEGIN
  SELECT RAISE(ABORT, 'invalid asset upload session transition');
END;

CREATE INDEX asset_upload_sessions_scope
ON asset_upload_sessions(account_id, script_name, status, expires_at_ms);

CREATE TABLE asset_upload_entries (
  session_id TEXT NOT NULL REFERENCES asset_upload_sessions(id),
  path TEXT NOT NULL,
  wrangler_hash TEXT NOT NULL CHECK(length(wrangler_hash) = 32),
  size INTEGER NOT NULL CHECK(size >= 0),
  content_type TEXT,
  artifact_sha256 BLOB CHECK(artifact_sha256 IS NULL OR length(artifact_sha256) = 32),
  uploaded_at_ms INTEGER,
  PRIMARY KEY(session_id, path),
  CHECK((artifact_sha256 IS NULL AND uploaded_at_ms IS NULL) OR
        (artifact_sha256 IS NOT NULL AND uploaded_at_ms IS NOT NULL))
) WITHOUT ROWID, STRICT;

CREATE TRIGGER asset_upload_entry_evidence_immutable
BEFORE UPDATE ON asset_upload_entries
WHEN NEW.session_id != OLD.session_id OR NEW.path != OLD.path
  OR NEW.wrangler_hash != OLD.wrangler_hash OR NEW.size != OLD.size
  OR (OLD.artifact_sha256 IS NOT NULL AND (
    NEW.artifact_sha256 IS NOT OLD.artifact_sha256
    OR NEW.content_type IS NOT OLD.content_type
    OR NEW.uploaded_at_ms IS NOT OLD.uploaded_at_ms
  ))
  OR (OLD.artifact_sha256 IS NULL AND NEW.artifact_sha256 IS NOT NULL AND (
    length(NEW.artifact_sha256) != 32 OR NEW.content_type IS NULL OR NEW.uploaded_at_ms IS NULL
  ))
BEGIN
  SELECT RAISE(ABORT, 'asset upload entry evidence is immutable');
END;

CREATE INDEX asset_upload_entries_hash
ON asset_upload_entries(session_id, wrangler_hash);

CREATE TABLE version_upload_objects (
  session_id TEXT NOT NULL REFERENCES version_uploads(id),
  sha256 BLOB NOT NULL CHECK(length(sha256) = 32),
  object_kind TEXT NOT NULL CHECK(object_kind IN ('bundle', 'asset_manifest', 'asset_blob')),
  size INTEGER NOT NULL CHECK(size >= 0),
  verified INTEGER NOT NULL DEFAULT 0 CHECK(verified IN (0, 1)),
  verified_at_ms INTEGER,
  PRIMARY KEY(session_id, sha256)
) WITHOUT ROWID, STRICT;

CREATE TRIGGER version_assets_insert_guard
BEFORE INSERT ON version_assets
WHEN (SELECT state FROM worker_versions WHERE id = NEW.version_id) != 'staging'
BEGIN
  SELECT RAISE(ABORT, 'immutable version assets');
END;

CREATE TRIGGER version_assets_update_guard
BEFORE UPDATE ON version_assets
BEGIN
  SELECT RAISE(ABORT, 'immutable version assets');
END;

CREATE TRIGGER version_assets_delete_guard
BEFORE DELETE ON version_assets
WHEN (SELECT state FROM worker_versions WHERE id = OLD.version_id) != 'deleting'
BEGIN
  SELECT RAISE(ABORT, 'immutable version assets');
END;

CREATE TRIGGER version_object_refs_insert_guard
BEFORE INSERT ON version_object_refs
WHEN (SELECT state FROM worker_versions WHERE id = NEW.version_id) != 'staging'
BEGIN
  SELECT RAISE(ABORT, 'immutable version object refs');
END;

CREATE TRIGGER version_object_refs_update_guard
BEFORE UPDATE ON version_object_refs
BEGIN
  SELECT RAISE(ABORT, 'immutable version object refs');
END;

CREATE TRIGGER version_object_refs_delete_guard
BEFORE DELETE ON version_object_refs
WHEN (SELECT state FROM worker_versions WHERE id = OLD.version_id) != 'deleting'
BEGIN
  SELECT RAISE(ABORT, 'immutable version object refs');
END;
