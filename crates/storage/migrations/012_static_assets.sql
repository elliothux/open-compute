CREATE TABLE deployment_assets (
  deployment_id TEXT PRIMARY KEY REFERENCES worker_deployments(id),
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

CREATE TABLE deployment_object_refs (
  deployment_id TEXT NOT NULL REFERENCES worker_deployments(id),
  object_kind TEXT NOT NULL CHECK(object_kind IN ('bundle', 'asset_manifest', 'asset_blob')),
  sha256 BLOB NOT NULL CHECK(length(sha256) = 32),
  size INTEGER NOT NULL CHECK(size >= 0),
  created_at_ms INTEGER NOT NULL,
  PRIMARY KEY(deployment_id, object_kind, sha256)
) WITHOUT ROWID, STRICT;

CREATE INDEX deployment_object_refs_digest
ON deployment_object_refs(sha256, deployment_id);

CREATE TABLE deployment_uploads (
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
  deployment_id TEXT,
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
    (status IN ('open', 'aborted', 'expired') AND deployment_id IS NULL
      AND finalize_fingerprint IS NULL AND finalize_owner_startup_id IS NULL
      AND finalize_response_json IS NULL AND finalize_error_code IS NULL) OR
    (status = 'finalizing' AND deployment_id IS NOT NULL
      AND finalize_fingerprint IS NOT NULL AND finalize_owner_startup_id IS NOT NULL
      AND finalize_response_json IS NULL AND finalize_error_code IS NULL) OR
    (status = 'committed' AND deployment_id IS NOT NULL
      AND finalize_fingerprint IS NOT NULL AND finalize_owner_startup_id IS NOT NULL
      AND ((finalize_response_json IS NOT NULL AND finalize_error_code IS NULL) OR
           (finalize_response_json IS NULL AND finalize_error_code IS NOT NULL)))
  ),
  UNIQUE(account_id, worker_id, idempotency_key)
) STRICT;

CREATE INDEX deployment_uploads_worker_status
ON deployment_uploads(account_id, worker_id, status, expires_at_ms);

CREATE TABLE deployment_upload_objects (
  session_id TEXT NOT NULL REFERENCES deployment_uploads(id),
  sha256 BLOB NOT NULL CHECK(length(sha256) = 32),
  object_kind TEXT NOT NULL CHECK(object_kind IN ('bundle', 'asset_manifest', 'asset_blob')),
  size INTEGER NOT NULL CHECK(size >= 0),
  verified INTEGER NOT NULL DEFAULT 0 CHECK(verified IN (0, 1)),
  verified_at_ms INTEGER,
  PRIMARY KEY(session_id, sha256)
) WITHOUT ROWID, STRICT;

CREATE TRIGGER deployment_assets_insert_guard
BEFORE INSERT ON deployment_assets
WHEN (SELECT state FROM worker_deployments WHERE id = NEW.deployment_id) != 'staging'
BEGIN
  SELECT RAISE(ABORT, 'immutable deployment assets');
END;

CREATE TRIGGER deployment_assets_update_guard
BEFORE UPDATE ON deployment_assets
BEGIN
  SELECT RAISE(ABORT, 'immutable deployment assets');
END;

CREATE TRIGGER deployment_assets_delete_guard
BEFORE DELETE ON deployment_assets
WHEN (SELECT state FROM worker_deployments WHERE id = OLD.deployment_id) != 'deleting'
BEGIN
  SELECT RAISE(ABORT, 'immutable deployment assets');
END;

CREATE TRIGGER deployment_object_refs_insert_guard
BEFORE INSERT ON deployment_object_refs
WHEN (SELECT state FROM worker_deployments WHERE id = NEW.deployment_id) != 'staging'
BEGIN
  SELECT RAISE(ABORT, 'immutable deployment object refs');
END;

CREATE TRIGGER deployment_object_refs_update_guard
BEFORE UPDATE ON deployment_object_refs
BEGIN
  SELECT RAISE(ABORT, 'immutable deployment object refs');
END;

CREATE TRIGGER deployment_object_refs_delete_guard
BEFORE DELETE ON deployment_object_refs
WHEN (SELECT state FROM worker_deployments WHERE id = OLD.deployment_id) != 'deleting'
BEGIN
  SELECT RAISE(ABORT, 'immutable deployment object refs');
END;
