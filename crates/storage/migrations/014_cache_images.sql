CREATE TABLE version_cache_policies (
  version_id TEXT NOT NULL REFERENCES worker_versions(id),
  entrypoint_name TEXT NOT NULL,
  enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
  cross_version_cache INTEGER NOT NULL CHECK(cross_version_cache IN (0, 1)),
  PRIMARY KEY(version_id, entrypoint_name),
  CHECK(length(entrypoint_name) <= 128)
) WITHOUT ROWID, STRICT;

CREATE TABLE version_builtin_bindings (
  version_id TEXT NOT NULL REFERENCES worker_versions(id),
  binding_name TEXT NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN ('ai', 'images', 'version_metadata')),
  tag TEXT,
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  PRIMARY KEY(version_id, binding_name),
  UNIQUE(version_id, kind),
  CHECK(length(binding_name) BETWEEN 1 AND 64),
  CHECK(tag IS NULL OR length(tag) BETWEEN 1 AND 128),
  CHECK((kind IN ('ai', 'images') AND tag IS NULL) OR kind = 'version_metadata')
) WITHOUT ROWID, STRICT;

CREATE TRIGGER version_cache_policies_insert_guard
BEFORE INSERT ON version_cache_policies
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM worker_versions
    WHERE id = NEW.version_id AND state = 'staging' AND content_kind = 'worker'
  ) THEN RAISE(ABORT, 'cache policy authority invariant') END;
  SELECT CASE WHEN NEW.entrypoint_name != '' AND (
    NEW.entrypoint_name GLOB '*[^A-Za-z0-9_$]*'
    OR NEW.entrypoint_name GLOB '[^A-Za-z_$]*'
  ) THEN RAISE(ABORT, 'cache entrypoint invariant') END;
END;

CREATE TRIGGER version_cache_policies_update_guard
BEFORE UPDATE ON version_cache_policies
BEGIN
  SELECT RAISE(ABORT, 'immutable version cache policy');
END;

CREATE TRIGGER version_cache_policies_delete_guard
BEFORE DELETE ON version_cache_policies
WHEN (SELECT state FROM worker_versions WHERE id = OLD.version_id)
  NOT IN ('staging', 'rejected', 'deleting')
BEGIN
  SELECT RAISE(ABORT, 'immutable version cache policy');
END;

CREATE TRIGGER version_builtin_bindings_insert_guard
BEFORE INSERT ON version_builtin_bindings
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM worker_versions
    WHERE id = NEW.version_id AND state = 'staging' AND content_kind = 'worker'
  ) THEN RAISE(ABORT, 'builtin binding authority invariant') END;
  SELECT CASE WHEN NEW.binding_name GLOB '*[^A-Za-z0-9_]*'
    OR NEW.binding_name GLOB '[^A-Za-z_]*'
    OR NEW.binding_name GLOB 'OPEN_COMPUTE_*'
    OR NEW.binding_name GLOB '__*'
  THEN RAISE(ABORT, 'builtin binding name invariant') END;
  SELECT CASE WHEN EXISTS (
    SELECT 1 FROM version_vars WHERE version_id = NEW.version_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM version_secrets WHERE version_id = NEW.version_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM version_bindings WHERE version_id = NEW.version_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM queue_producer_bindings WHERE version_id = NEW.version_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM workflow_bindings WHERE version_id = NEW.version_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM version_services WHERE version_id = NEW.version_id AND binding_name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM version_assets WHERE version_id = NEW.version_id AND binding_name = NEW.binding_name
  ) THEN RAISE(ABORT, 'builtin binding env name conflict') END;
END;

CREATE TRIGGER version_builtin_bindings_update_guard
BEFORE UPDATE ON version_builtin_bindings
BEGIN
  SELECT RAISE(ABORT, 'immutable version builtin binding');
END;

CREATE TRIGGER version_builtin_bindings_delete_guard
BEFORE DELETE ON version_builtin_bindings
WHEN (SELECT state FROM worker_versions WHERE id = OLD.version_id)
  NOT IN ('staging', 'rejected', 'deleting')
BEGIN
  SELECT RAISE(ABORT, 'immutable version builtin binding');
END;
