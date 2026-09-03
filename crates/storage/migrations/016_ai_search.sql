CREATE TABLE ai_search_namespaces (
  resource_id    TEXT PRIMARY KEY REFERENCES resources(id),
  description    TEXT CHECK(description IS NULL OR length(description) <= 256),
  created_at_ms  INTEGER NOT NULL
) STRICT;

CREATE TABLE ai_search_instances (
  resource_id            TEXT PRIMARY KEY REFERENCES resources(id),
  namespace_resource_id  TEXT NOT NULL REFERENCES ai_search_namespaces(resource_id),
  instance_key           TEXT NOT NULL CHECK(
    length(CAST(instance_key AS BLOB)) BETWEEN 1 AND 64
    AND instance_key NOT GLOB '*[' || char(0) || '-' || char(31) || ']*'
  ),
  storage_key            TEXT NOT NULL UNIQUE,
  schema_version         INTEGER NOT NULL CHECK(schema_version = 1),
  model_contract_sha256  BLOB NOT NULL CHECK(length(model_contract_sha256) = 32),
  created_at_ms          INTEGER NOT NULL,
  UNIQUE(namespace_resource_id, instance_key)
) STRICT;

CREATE TRIGGER ai_search_namespace_insert_guard
BEFORE INSERT ON ai_search_namespaces
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM resources
    WHERE id = NEW.resource_id
      AND kind = 'ai_search_namespace'
      AND state = 'creating'
      AND driver_schema_version = 1
      AND created_at_ms = NEW.created_at_ms
  ) THEN RAISE(ABORT, 'AI Search namespace authority invariant') END;
END;

CREATE TRIGGER ai_search_namespace_identity_immutable_guard
BEFORE UPDATE OF resource_id, created_at_ms ON ai_search_namespaces
BEGIN
  SELECT RAISE(ABORT, 'immutable AI Search namespace identity');
END;

CREATE TRIGGER ai_search_instance_insert_guard
BEFORE INSERT ON ai_search_instances
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1
    FROM resources child
    JOIN resources parent
      ON parent.id = NEW.namespace_resource_id
     AND parent.account_id = child.account_id
    JOIN ai_search_namespaces namespace
      ON namespace.resource_id = parent.id
    WHERE child.id = NEW.resource_id
      AND child.kind = 'ai_search_instance'
      AND child.state = 'creating'
      AND child.driver_schema_version = NEW.schema_version
      AND child.created_at_ms = NEW.created_at_ms
      AND parent.kind = 'ai_search_namespace'
      AND parent.state = 'ready'
  ) THEN RAISE(ABORT, 'AI Search instance authority invariant') END;
END;

CREATE TRIGGER ai_search_instance_identity_immutable_guard
BEFORE UPDATE OF resource_id, namespace_resource_id, instance_key, storage_key,
                 schema_version, created_at_ms ON ai_search_instances
BEGIN
  SELECT RAISE(ABORT, 'immutable AI Search instance identity');
END;

CREATE TRIGGER ai_search_instance_referrer_insert
AFTER INSERT ON ai_search_instances
BEGIN
  INSERT INTO resource_referrers(resource_id, referrer_kind, referrer_id, created_at_ms)
  VALUES(NEW.namespace_resource_id, 'ai_search_instance', NEW.resource_id, NEW.created_at_ms);
END;

CREATE TRIGGER ai_search_instance_referrer_delete
AFTER DELETE ON ai_search_instances
BEGIN
  DELETE FROM resource_referrers
  WHERE resource_id = OLD.namespace_resource_id
    AND referrer_kind = 'ai_search_instance'
    AND referrer_id = OLD.resource_id;
END;

CREATE TRIGGER ai_search_instance_referrer_insert_guard
BEFORE INSERT ON resource_referrers
WHEN NEW.referrer_kind = 'ai_search_instance'
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM ai_search_instances
    WHERE resource_id = NEW.referrer_id
      AND namespace_resource_id = NEW.resource_id
      AND created_at_ms = NEW.created_at_ms
  ) THEN RAISE(ABORT, 'orphan AI Search instance referrer') END;
END;

CREATE TRIGGER ai_search_instance_referrer_delete_guard
BEFORE DELETE ON resource_referrers
WHEN OLD.referrer_kind = 'ai_search_instance'
 AND EXISTS (
   SELECT 1 FROM ai_search_instances child_locator
   JOIN resources child ON child.id = child_locator.resource_id
   WHERE child_locator.resource_id = OLD.referrer_id
     AND child_locator.namespace_resource_id = OLD.resource_id
     AND child.state != 'tombstoned'
 )
BEGIN
  SELECT RAISE(ABORT, 'live AI Search instance referrer');
END;

CREATE TRIGGER ai_search_namespace_delete_guard
BEFORE DELETE ON ai_search_namespaces
WHEN (SELECT state FROM resources WHERE id = OLD.resource_id)
  NOT IN ('deleting', 'tombstoned')
BEGIN
  SELECT RAISE(ABORT, 'live AI Search namespace locator');
END;

CREATE TRIGGER ai_search_instance_delete_guard
BEFORE DELETE ON ai_search_instances
WHEN (SELECT state FROM resources WHERE id = OLD.resource_id)
  NOT IN ('deleting', 'tombstoned')
BEGIN
  SELECT RAISE(ABORT, 'live AI Search instance locator');
END;

CREATE TRIGGER ai_search_namespace_child_delete_fence
BEFORE UPDATE OF state ON resources
WHEN OLD.kind = 'ai_search_namespace'
 AND NEW.state IN ('deleting', 'tombstoned')
 AND EXISTS (
   SELECT 1
   FROM ai_search_instances child_locator
   JOIN resources child ON child.id = child_locator.resource_id
   WHERE child_locator.namespace_resource_id = OLD.id
     AND child.state != 'tombstoned'
 )
BEGIN
  SELECT RAISE(ABORT, 'AI Search namespace still has live instances');
END;

CREATE TRIGGER ai_search_resource_tombstone_retire_instance
AFTER UPDATE OF state ON resources
WHEN NEW.state = 'tombstoned' AND NEW.kind = 'ai_search_instance'
BEGIN
  DELETE FROM ai_search_instances WHERE resource_id = NEW.id;
END;

CREATE TRIGGER ai_search_resource_tombstone_retire_namespace
AFTER UPDATE OF state ON resources
WHEN NEW.state = 'tombstoned' AND NEW.kind = 'ai_search_namespace'
BEGIN
  DELETE FROM ai_search_namespaces WHERE resource_id = NEW.id;
END;
