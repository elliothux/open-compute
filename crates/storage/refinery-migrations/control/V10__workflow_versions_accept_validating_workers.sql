DROP TRIGGER workflow_version_insert_guard;

CREATE TRIGGER workflow_version_insert_guard BEFORE INSERT ON workflow_versions
BEGIN
  SELECT CASE WHEN NEW.state != 'staging' OR NOT EXISTS (
    SELECT 1 FROM workflow_definitions f JOIN workers w ON w.id = NEW.worker_id
    JOIN worker_versions d ON d.worker_id = w.id
    WHERE f.id = NEW.definition_id AND f.state IN ('creating','ready')
      AND w.id = NEW.worker_id AND w.deleted_at_ms IS NULL
      AND d.id = NEW.worker_version_id
      AND (d.state = 'ready' OR (d.state = 'validating' AND NEW.reservation_owner IS NOT NULL))
      AND d.worker_code_sha256 = NEW.worker_code_sha256 AND d.loader_schema_version = NEW.loader_schema_version
  ) THEN RAISE(ABORT,'workflow version authority') END;
END;
