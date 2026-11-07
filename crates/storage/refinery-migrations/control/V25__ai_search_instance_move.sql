-- Cloudflare moves an AI Search instance between namespaces of one account.
-- Only the parent link may change; the per-instance identity stays immutable.
DROP TRIGGER ai_search_instance_identity_immutable_guard;
CREATE TRIGGER ai_search_instance_identity_immutable_guard
BEFORE UPDATE OF resource_id, instance_key, storage_key,
                 schema_version, created_at_ms ON ai_search_instances
BEGIN
  SELECT RAISE(ABORT, 'immutable AI Search instance identity');
END;
CREATE TRIGGER ai_search_instance_move_guard
BEFORE UPDATE OF namespace_resource_id ON ai_search_instances
WHEN NEW.namespace_resource_id != OLD.namespace_resource_id
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1
    FROM resources child
    JOIN resources parent
      ON parent.id = NEW.namespace_resource_id
    JOIN ai_search_namespaces namespace
      ON namespace.resource_id = parent.id
    WHERE child.id = NEW.resource_id
      AND child.kind = 'ai_search_instance'
      AND child.state = 'ready'
      AND parent.kind = 'ai_search_namespace'
      AND parent.state = 'ready'
  ) THEN RAISE(ABORT, 'AI Search instance authority invariant') END;
END;
CREATE TRIGGER ai_search_instance_referrer_move
AFTER UPDATE OF namespace_resource_id ON ai_search_instances
WHEN NEW.namespace_resource_id != OLD.namespace_resource_id
BEGIN
  DELETE FROM resource_referrers
  WHERE resource_id = OLD.namespace_resource_id
    AND referrer_kind = 'ai_search_instance'
    AND referrer_id = OLD.resource_id;
  INSERT INTO resource_referrers(resource_id, referrer_kind, referrer_id, created_at_ms)
  VALUES(NEW.namespace_resource_id, 'ai_search_instance', NEW.resource_id, NEW.created_at_ms);
END;
