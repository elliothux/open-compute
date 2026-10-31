-- Dashboard files now come from the binary, independent of the Worker catalog.
DROP TABLE system_owned_versions;

UPDATE workers SET active_deployment_id = NULL WHERE id IN (
  SELECT id FROM workers WHERE ownership = 'system' AND name = 'open-compute-dashboard'
);
-- Follow the existing transitions so immutable asset metadata can be deleted.
UPDATE worker_versions SET state = 'rejected'
WHERE worker_id IN (
  SELECT id FROM workers WHERE ownership = 'system' AND name = 'open-compute-dashboard'
) AND state IN ('staging', 'validating');
UPDATE worker_versions SET state = 'deleting'
WHERE worker_id IN (
  SELECT id FROM workers WHERE ownership = 'system' AND name = 'open-compute-dashboard'
) AND state IN ('ready', 'rejected');

DELETE FROM control_idempotency
WHERE idempotency_key GLOB 'system-dashboard:*' OR version_id IN (SELECT id FROM worker_versions WHERE worker_id IN (
  SELECT id FROM workers WHERE ownership = 'system' AND name = 'open-compute-dashboard'
));
DELETE FROM control_audit_events
WHERE (target_type = 'worker' AND target_id IN (
  SELECT id FROM workers WHERE ownership = 'system' AND name = 'open-compute-dashboard'
))
   OR (target_type = 'version' AND target_id IN (SELECT id FROM worker_versions WHERE worker_id IN (
  SELECT id FROM workers WHERE ownership = 'system' AND name = 'open-compute-dashboard'
)))
   OR (target_type = 'deployment' AND target_id IN (SELECT id FROM worker_deployments WHERE worker_id IN (
  SELECT id FROM workers WHERE ownership = 'system' AND name = 'open-compute-dashboard'
)));
DELETE FROM version_upload_objects
WHERE session_id IN (SELECT id FROM version_uploads WHERE worker_id IN (
  SELECT id FROM workers WHERE ownership = 'system' AND name = 'open-compute-dashboard'
));
DELETE FROM version_uploads WHERE worker_id IN (
  SELECT id FROM workers WHERE ownership = 'system' AND name = 'open-compute-dashboard'
);
DELETE FROM deployment_runtime_assessments WHERE deployment_id IN (SELECT id FROM worker_deployments WHERE worker_id IN (
  SELECT id FROM workers WHERE ownership = 'system' AND name = 'open-compute-dashboard'
));
DELETE FROM worker_deployments WHERE worker_id IN (
  SELECT id FROM workers WHERE ownership = 'system' AND name = 'open-compute-dashboard'
);

-- The route-to-claim FK is deferred; retain the route IDs while deleting claims.
DELETE FROM hostname_claims
WHERE id IN (SELECT claim_id FROM worker_host_routes WHERE worker_id IN (
  SELECT id FROM workers WHERE ownership = 'system' AND name = 'open-compute-dashboard'
));
DELETE FROM worker_host_routes WHERE worker_id IN (
  SELECT id FROM workers WHERE ownership = 'system' AND name = 'open-compute-dashboard'
);
DELETE FROM worker_delete_intents WHERE worker_id IN (
  SELECT id FROM workers WHERE ownership = 'system' AND name = 'open-compute-dashboard'
);

-- The system Dashboard has only static assets, with no product bindings or triggers.
DELETE FROM version_annotations WHERE version_id IN (SELECT id FROM worker_versions WHERE worker_id IN (
  SELECT id FROM workers WHERE ownership = 'system' AND name = 'open-compute-dashboard'
));
DELETE FROM version_vars WHERE version_id IN (SELECT id FROM worker_versions WHERE worker_id IN (
  SELECT id FROM workers WHERE ownership = 'system' AND name = 'open-compute-dashboard'
));
DELETE FROM version_secrets WHERE version_id IN (SELECT id FROM worker_versions WHERE worker_id IN (
  SELECT id FROM workers WHERE ownership = 'system' AND name = 'open-compute-dashboard'
));
DELETE FROM version_assets WHERE version_id IN (SELECT id FROM worker_versions WHERE worker_id IN (
  SELECT id FROM workers WHERE ownership = 'system' AND name = 'open-compute-dashboard'
));
DELETE FROM version_object_refs WHERE version_id IN (SELECT id FROM worker_versions WHERE worker_id IN (
  SELECT id FROM workers WHERE ownership = 'system' AND name = 'open-compute-dashboard'
));
DELETE FROM version_cache_policies WHERE version_id IN (SELECT id FROM worker_versions WHERE worker_id IN (
  SELECT id FROM workers WHERE ownership = 'system' AND name = 'open-compute-dashboard'
));
DELETE FROM version_referrers WHERE version_id IN (SELECT id FROM worker_versions WHERE worker_id IN (
  SELECT id FROM workers WHERE ownership = 'system' AND name = 'open-compute-dashboard'
));
DELETE FROM worker_versions WHERE worker_id IN (
  SELECT id FROM workers WHERE ownership = 'system' AND name = 'open-compute-dashboard'
);
-- Worker observability settings use ON DELETE CASCADE.
DELETE FROM workers WHERE id IN (
  SELECT id FROM workers WHERE ownership = 'system' AND name = 'open-compute-dashboard'
);
