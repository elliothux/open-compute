DROP TRIGGER ai_search_instance_insert_guard;
DROP TRIGGER ai_search_instance_identity_immutable_guard;
DROP TRIGGER ai_search_instance_referrer_insert;
DROP TRIGGER ai_search_instance_referrer_delete;
DROP TRIGGER ai_search_instance_referrer_insert_guard;
DROP TRIGGER ai_search_instance_referrer_delete_guard;
DROP TRIGGER ai_search_instance_delete_guard;
DROP TRIGGER ai_search_namespace_child_delete_fence;
DROP TRIGGER ai_search_resource_tombstone_retire_instance;

ALTER TABLE ai_search_instances RENAME TO ai_search_instances_v1;

CREATE TABLE ai_search_instances (
  resource_id            TEXT PRIMARY KEY REFERENCES resources(id),
  namespace_resource_id  TEXT NOT NULL REFERENCES ai_search_namespaces(resource_id),
  instance_key           TEXT NOT NULL CHECK(
    length(CAST(instance_key AS BLOB)) BETWEEN 1 AND 64
    AND instance_key NOT GLOB '*[' || char(0) || '-' || char(31) || ']*'
  ),
  storage_key            TEXT NOT NULL UNIQUE,
  schema_version         INTEGER NOT NULL CHECK(schema_version IN (1, 2)),
  model_contract_sha256  BLOB NOT NULL CHECK(length(model_contract_sha256) = 32),
  created_at_ms          INTEGER NOT NULL,
  UNIQUE(namespace_resource_id, instance_key)
) STRICT;

INSERT INTO ai_search_instances
SELECT * FROM ai_search_instances_v1;

DROP TABLE ai_search_instances_v1;

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

CREATE TABLE ai_search_r2_sources (
  instance_resource_id  TEXT PRIMARY KEY REFERENCES ai_search_instances(resource_id) ON DELETE CASCADE,
  bucket_resource_id    TEXT NOT NULL REFERENCES r2_buckets(resource_id),
  bucket_name           TEXT NOT NULL CHECK(length(CAST(bucket_name AS BLOB)) BETWEEN 1 AND 512),
  created_at_ms          INTEGER NOT NULL
) STRICT;

CREATE INDEX ai_search_r2_sources_bucket
ON ai_search_r2_sources(bucket_resource_id, instance_resource_id);

CREATE TRIGGER ai_search_r2_source_insert_guard
BEFORE INSERT ON ai_search_r2_sources
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1
      FROM ai_search_instances instance
      JOIN resources child ON child.id = instance.resource_id
      JOIN resources bucket ON bucket.id = NEW.bucket_resource_id
      JOIN r2_buckets r2 ON r2.resource_id = bucket.id
     WHERE instance.resource_id = NEW.instance_resource_id
       AND child.kind = 'ai_search_instance'
       AND child.state = 'creating'
       AND child.account_id = bucket.account_id
       AND bucket.kind = 'r2_bucket'
       AND bucket.state = 'ready'
       AND bucket.availability = 'healthy'
       AND bucket.name = NEW.bucket_name
  ) THEN RAISE(ABORT, 'AI Search R2 source authority invariant') END;
END;

CREATE TRIGGER ai_search_r2_source_immutable_guard
BEFORE UPDATE ON ai_search_r2_sources
BEGIN
  SELECT RAISE(ABORT, 'immutable AI Search R2 source');
END;

CREATE TRIGGER ai_search_r2_source_bucket_delete_guard
BEFORE UPDATE OF state ON resources
WHEN OLD.state != 'deleting' AND NEW.state = 'deleting'
  AND EXISTS (
    SELECT 1 FROM ai_search_r2_sources
     WHERE bucket_resource_id = OLD.id
  )
BEGIN
  SELECT RAISE(ABORT, 'R2 bucket is an AI Search source');
END;
