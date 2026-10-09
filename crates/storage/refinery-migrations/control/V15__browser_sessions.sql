-- Extend the current builtin authority without changing published migrations.
CREATE TABLE version_builtin_bindings_next (
  version_id TEXT NOT NULL REFERENCES worker_versions(id),
  binding_name TEXT NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN (
    'worker_loader', 'ai', 'images', 'browser', 'version_metadata', 'wasm_module', 'text_blob', 'data_blob'
  )),
  tag TEXT,
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  PRIMARY KEY(version_id, binding_name),
  CHECK(length(binding_name) BETWEEN 1 AND 64),
  CHECK(tag IS NULL OR length(tag) BETWEEN 1 AND 1024),
  CHECK(
    (kind IN ('worker_loader', 'ai', 'images', 'browser') AND tag IS NULL)
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

-- Browser locators remain private generation-local execution indexes, never durable secrets.
CREATE TABLE browser_sessions (
  id TEXT PRIMARY KEY CHECK(length(id) = 36),
  instance_id TEXT NOT NULL REFERENCES instance_identity(instance_id),
  runtime_generation TEXT NOT NULL CHECK(length(runtime_generation) = 36),
  runtime_contract_sha256 BLOB NOT NULL CHECK(length(runtime_contract_sha256) = 32),
  state TEXT NOT NULL CHECK(state IN ('ready', 'connected', 'closing', 'closed', 'lost')),
  keep_alive_ms INTEGER NOT NULL CHECK(keep_alive_ms BETWEEN 10000 AND 1200000),
  connections INTEGER NOT NULL DEFAULT 0 CHECK(connections >= 0),
  created_at_ms INTEGER NOT NULL,
  connected_at_ms INTEGER,
  last_activity_at_ms INTEGER NOT NULL,
  closed_at_ms INTEGER,
  close_reason TEXT CHECK(close_reason IN ('normal', 'idle', 'lost')),
  CHECK((state IN ('closed', 'lost')) = (close_reason IS NOT NULL)),
  CHECK((state = 'lost') = (close_reason = 'lost') OR close_reason IS NULL),
  CHECK((state IN ('closed', 'lost')) = (closed_at_ms IS NOT NULL)),
  CHECK(state != 'connected' OR connections > 0),
  CHECK(state != 'ready' OR connections = 0),
  CHECK(state NOT IN ('closed', 'lost') OR connections = 0)
) WITHOUT ROWID, STRICT;

CREATE INDEX browser_sessions_generation ON browser_sessions(runtime_generation, state);
CREATE INDEX browser_sessions_history ON browser_sessions(instance_id, created_at_ms, id);
CREATE INDEX browser_sessions_terminal_retention ON browser_sessions(instance_id, closed_at_ms DESC, id DESC)
WHERE state IN ('closed', 'lost');

CREATE TRIGGER browser_sessions_identity_guard
BEFORE UPDATE ON browser_sessions
WHEN NEW.id != OLD.id OR NEW.instance_id != OLD.instance_id
  OR NEW.runtime_generation != OLD.runtime_generation
  OR NEW.runtime_contract_sha256 != OLD.runtime_contract_sha256
  OR NEW.created_at_ms != OLD.created_at_ms OR NEW.keep_alive_ms != OLD.keep_alive_ms
BEGIN
  SELECT RAISE(ABORT, 'immutable browser session identity');
END;

CREATE TRIGGER browser_sessions_terminal_guard
BEFORE UPDATE ON browser_sessions
WHEN OLD.state IN ('closed', 'lost')
BEGIN
  SELECT RAISE(ABORT, 'terminal browser session');
END;
