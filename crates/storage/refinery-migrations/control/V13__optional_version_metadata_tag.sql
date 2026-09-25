-- A Version Metadata binding has no tag until the Worker Version is tagged.
-- The published V1 table incorrectly required a tag even for an untagged Version.
CREATE TABLE version_builtin_bindings_next (
  version_id TEXT NOT NULL REFERENCES worker_versions(id),
  binding_name TEXT NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN (
    'worker_loader', 'ai', 'images', 'version_metadata', 'wasm_module', 'text_blob', 'data_blob'
  )),
  tag TEXT,
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  PRIMARY KEY(version_id, binding_name),
  CHECK(length(binding_name) BETWEEN 1 AND 64),
  CHECK(tag IS NULL OR length(tag) BETWEEN 1 AND 1024),
  CHECK(
    (kind IN ('worker_loader', 'ai', 'images') AND tag IS NULL)
    OR kind = 'version_metadata'
    OR (kind IN ('wasm_module', 'text_blob', 'data_blob') AND tag IS NOT NULL)
  )
) WITHOUT ROWID, STRICT;

INSERT INTO version_builtin_bindings_next
  (version_id, binding_name, kind, tag, descriptor_sha256)
SELECT version_id, binding_name, kind, tag, descriptor_sha256
FROM version_builtin_bindings;
DROP TABLE version_builtin_bindings;
ALTER TABLE version_builtin_bindings_next RENAME TO version_builtin_bindings;

CREATE UNIQUE INDEX version_builtin_bindings_singleton_kind
ON version_builtin_bindings(version_id, kind)
WHERE kind IN ('ai', 'images', 'version_metadata');

CREATE TRIGGER version_builtin_bindings_insert_guard
BEFORE INSERT ON version_builtin_bindings
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM worker_versions
    WHERE id = NEW.version_id AND state = 'staging' AND content_kind = 'worker'
  ) THEN RAISE(ABORT, 'builtin binding authority invariant') END;
  SELECT CASE WHEN NEW.binding_name GLOB '*[^A-Za-z0-9_$]*'
    OR NEW.binding_name GLOB '[^A-Za-z_$]*'
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
    SELECT 1 FROM queue_producer_bindings
    WHERE version_id = NEW.version_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM workflow_bindings WHERE version_id = NEW.version_id AND name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM version_services
    WHERE version_id = NEW.version_id AND binding_name = NEW.binding_name
  ) OR EXISTS (
    SELECT 1 FROM version_assets
    WHERE version_id = NEW.version_id AND binding_name = NEW.binding_name
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
