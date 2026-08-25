CREATE TABLE do_namespaces (
  resource_id           TEXT PRIMARY KEY REFERENCES resources(id),
  owner_worker_id       TEXT NOT NULL REFERENCES workers(id),
  class_name            TEXT NOT NULL,
  do_storage_id         TEXT NOT NULL,
  namespace_storage_key TEXT NOT NULL UNIQUE,
  schema_version        INTEGER NOT NULL CHECK(schema_version >= 1),
  created_at_ms         INTEGER NOT NULL,
  CHECK(length(class_name) BETWEEN 1 AND 128),
  CHECK(class_name NOT GLOB '*[^A-Za-z0-9_$]*'),
  CHECK(class_name NOT GLOB '[^A-Za-z_$]*'),
  CHECK(length(do_storage_id) BETWEEN 1 AND 128),
  CHECK(length(namespace_storage_key) = 64 AND namespace_storage_key = lower(namespace_storage_key)),
  UNIQUE(owner_worker_id, class_name)
) STRICT;

CREATE TABLE do_objects (
  namespace_resource_id TEXT NOT NULL REFERENCES do_namespaces(resource_id) ON DELETE CASCADE,
  object_id             TEXT NOT NULL,
  generation            INTEGER NOT NULL CHECK(generation >= 1),
  state                 TEXT NOT NULL CHECK(state IN (
                          'creating', 'ready', 'deleting', 'tombstoned'
                        )),
  created_at_ms         INTEGER NOT NULL,
  updated_at_ms         INTEGER NOT NULL,
  deleted_at_ms         INTEGER,
  PRIMARY KEY(namespace_resource_id, object_id, generation),
  CHECK(length(object_id) = 64 AND object_id = lower(object_id)),
  CHECK(object_id NOT GLOB '*[^0-9a-f]*'),
  CHECK((state = 'tombstoned') = (deleted_at_ms IS NOT NULL))
) STRICT;

CREATE UNIQUE INDEX do_objects_live_identity
ON do_objects(namespace_resource_id, object_id)
WHERE state != 'tombstoned';

CREATE INDEX do_objects_reconcile
ON do_objects(state, updated_at_ms, namespace_resource_id, object_id)
WHERE state IN ('creating', 'deleting');

CREATE TRIGGER do_namespace_insert_guard
BEFORE INSERT ON do_namespaces
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1
    FROM resources r
    JOIN workers w ON w.id = NEW.owner_worker_id
    WHERE r.id = NEW.resource_id
      AND r.kind = 'do_namespace'
      AND r.state = 'creating'
      AND r.account_id = w.account_id
      AND w.deleted_at_ms IS NULL
      AND w.do_storage_id = NEW.do_storage_id
      AND r.created_at_ms = NEW.created_at_ms
  ) THEN RAISE(ABORT, 'durable object namespace authority invariant') END;
END;

CREATE TRIGGER do_namespace_identity_immutable_guard
BEFORE UPDATE ON do_namespaces
BEGIN
  SELECT RAISE(ABORT, 'immutable durable object namespace identity');
END;

CREATE TRIGGER do_namespace_delete_guard
BEFORE DELETE ON do_namespaces
WHEN (SELECT state FROM resources WHERE id = OLD.resource_id) != 'tombstoned'
BEGIN
  SELECT RAISE(ABORT, 'live durable object namespace locator');
END;

CREATE TRIGGER do_object_insert_guard
BEFORE INSERT ON do_objects
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM resources r
    WHERE r.id = NEW.namespace_resource_id
      AND r.kind = 'do_namespace'
      AND r.state IN ('ready', 'deleting')
  ) THEN RAISE(ABORT, 'durable object registry authority invariant') END;
  SELECT CASE WHEN EXISTS (
    SELECT 1 FROM do_objects old
    WHERE old.namespace_resource_id = NEW.namespace_resource_id
      AND old.object_id = NEW.object_id
      AND old.generation >= NEW.generation
  ) THEN RAISE(ABORT, 'durable object generation invariant') END;
END;

CREATE TRIGGER do_object_identity_immutable_guard
BEFORE UPDATE OF namespace_resource_id, object_id, generation, created_at_ms ON do_objects
BEGIN
  SELECT RAISE(ABORT, 'immutable durable object identity');
END;

CREATE TRIGGER do_object_transition_guard
BEFORE UPDATE OF state ON do_objects
WHEN OLD.state != NEW.state AND NOT (
  (OLD.state = 'creating' AND NEW.state IN ('ready', 'deleting')) OR
  (OLD.state = 'ready' AND NEW.state = 'deleting') OR
  (OLD.state = 'deleting' AND NEW.state = 'tombstoned')
)
BEGIN
  SELECT RAISE(ABORT, 'invalid durable object transition');
END;

CREATE TRIGGER do_object_tombstone_guard
BEFORE UPDATE ON do_objects
WHEN OLD.state = 'tombstoned'
BEGIN
  SELECT RAISE(ABORT, 'immutable durable object tombstone');
END;

CREATE TRIGGER do_resource_tombstone_retire_locator
AFTER UPDATE OF state ON resources
WHEN NEW.state = 'tombstoned' AND NEW.kind = 'do_namespace'
BEGIN
  DELETE FROM do_namespaces WHERE resource_id = NEW.id;
END;
