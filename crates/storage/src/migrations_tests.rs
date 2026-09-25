use super::*;
use open_compute_core::{DeterministicClock, InstanceId, ResourceId};
use rusqlite::{Connection, params};
use std::time::UNIX_EPOCH;

#[test]
fn published_account_rows_become_one_instance_or_fail_atomically() {
    for (
        identity_mismatch,
        audit_mismatch,
        idempotency_mismatch,
        r2_mismatch,
        multipart_mismatch,
        asset_mismatch,
        version_upload_mismatch,
        resource_mismatch,
        artifact_mismatch,
        route_mismatch,
    ) in [
        (
            false, false, false, false, false, false, false, false, false, false,
        ),
        (
            true, false, false, false, false, false, false, false, false, false,
        ),
        (
            false, true, false, false, false, false, false, false, false, false,
        ),
        (
            false, false, true, false, false, false, false, false, false, false,
        ),
        (
            false, false, false, true, false, false, false, false, false, false,
        ),
        (
            false, false, false, false, true, false, false, false, false, false,
        ),
        (
            false, false, false, false, false, true, false, false, false, false,
        ),
        (
            false, false, false, false, false, false, true, false, false, false,
        ),
        (
            false, false, false, false, false, false, false, true, false, false,
        ),
        (
            false, false, false, false, false, false, false, false, true, false,
        ),
        (
            false, false, false, false, false, false, false, false, false, true,
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("control.sqlite");
        let mut connection = Connection::open(&path).unwrap();
        schema_migrations::migrate_to_for_test(&mut connection, DatabaseKind::Control, 8);
        let id = InstanceId::generate().to_string();
        connection
            .execute(
                "INSERT INTO accounts(id, name, created_at_ms, deleted_at_ms)
                 VALUES(?1, 'default', 7, NULL)",
                [&id],
            )
            .unwrap();
        let worker = open_compute_core::WorkerId::generate().to_string();
        connection
            .execute(
                "INSERT INTO workers
                 (id, account_id, name, active_deployment_id, do_storage_id,
                  route_generation, created_at_ms, updated_at_ms, deleted_at_ms, ownership)
                 VALUES(?1, ?2, 'app', NULL, ?3, 0, 7, 7, NULL, 'system')",
                rusqlite::params![
                    worker,
                    id,
                    open_compute_core::WorkerId::generate().to_string()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO hostname_claims
                 (id, hostname_ascii, account_id, namespace, exposure, state, generation,
                  created_at_ms, updated_at_ms, deleted_at_ms)
                 VALUES('route', ?1, ?2, 'worker', 'local', 'active', 1, 7, 7, NULL)",
                rusqlite::params![format!("app.{id}.localhost"), id],
            )
            .unwrap();
        if route_mismatch {
            connection
                .pragma_update(None, "foreign_keys", "OFF")
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO worker_host_routes
                 (id, claim_id, account_id, worker_id, namespace, exposure, path_prefix,
                  entrypoint, state, generation, created_at_ms, updated_at_ms, deleted_at_ms)
                 VALUES('route', 'route', ?1, ?2, 'worker', 'local', '/', NULL,
                        'active', 1, 7, 7, NULL)",
                rusqlite::params![
                    if route_mismatch {
                        InstanceId::generate().to_string()
                    } else {
                        id.clone()
                    },
                    worker
                ],
            )
            .unwrap();
        if route_mismatch {
            connection
                .pragma_update(None, "foreign_keys", "ON")
                .unwrap();
        }
        let workflow = open_compute_core::WorkflowId::generate().to_string();
        connection
            .execute(
                "INSERT INTO workflow_definitions
                 (id, account_id, name, state, availability, availability_code,
                  lifecycle_generation, created_at_ms, updated_at_ms)
                 VALUES(?1, ?2, 'flow', 'creating', 'degraded', 'WORKFLOW_VERSION_NOT_READY', 1, 7, 7)",
                rusqlite::params![workflow, id],
            )
            .unwrap();
        let audit_id = if audit_mismatch {
            InstanceId::generate().to_string()
        } else {
            id.clone()
        };
        connection
            .execute(
                "INSERT INTO control_audit_events
                 (account_id, action, target_type, target_id, request_id, details_json, created_at_ms)
                 VALUES (?1, 'worker.create', 'worker', ?2, 'request', X'7B7D', 7)",
                rusqlite::params![audit_id, worker],
            )
            .unwrap();
        let idempotency_id = if idempotency_mismatch {
            InstanceId::generate().to_string()
        } else {
            id.clone()
        };
        connection
            .execute(
                "INSERT INTO control_idempotency
                 (account_id, scope, idempotency_key, fingerprint_key_id,
                  request_fingerprint, response_json, state, created_at_ms, expires_at_ms)
                 VALUES (?1, 'worker.create', 'key', 'key-id', ?2, X'7B7D', 'complete', 7, 8)",
                rusqlite::params![idempotency_id, vec![9_u8; 32]],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO system_owned_versions
                 (kind, account_id, worker_id, active_version_id, assets_sha256, updated_at_ms)
                 VALUES('dashboard', ?1, ?2, NULL, zeroblob(32), 7)",
                rusqlite::params![id, worker],
            )
            .unwrap();
        let bucket = ResourceId::generate().to_string();
        let resource_owner = if resource_mismatch {
            connection
                .pragma_update(None, "foreign_keys", "OFF")
                .unwrap();
            InstanceId::generate().to_string()
        } else {
            id.clone()
        };
        connection
            .execute(
                "INSERT INTO resources
                 (id, account_id, kind, name, state, driver_schema_version, created_at_ms, updated_at_ms)
                 VALUES(?1, ?2, 'r2_bucket', 'bucket', 'creating', 1, 7, 7)",
                rusqlite::params![bucket, resource_owner],
            )
            .unwrap();
        if resource_mismatch {
            connection
                .pragma_update(None, "foreign_keys", "ON")
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO r2_buckets
                 (resource_id, physical_prefix, schema_version, max_object_bytes,
                  object_authority_sha256, created_at_ms)
                 VALUES(?1, ?2, 1, 100, zeroblob(32), 7)",
                rusqlite::params![bucket, format!("tenant/r2/v1/{bucket}/")],
            )
            .unwrap();
        let object_owner = if r2_mismatch {
            InstanceId::generate().to_string()
        } else {
            id.clone()
        };
        connection
            .execute(
                "INSERT INTO r2_objects
                 (resource_id, object_key, account_id, object_version, updated_at_ms)
                 VALUES(?1, 'committed', ?2, 'v1', 7)",
                rusqlite::params![bucket, object_owner],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO r2_object_mutations
                 (resource_id, object_key, account_id, kind, pending_version, started_at_ms)
                 VALUES(?1, 'pending', ?2, 'put', 'v2', 7)",
                rusqlite::params![bucket, object_owner],
            )
            .unwrap();
        let multipart_owner = if multipart_mismatch {
            InstanceId::generate().to_string()
        } else {
            id.clone()
        };
        connection
            .execute(
                "INSERT INTO r2_multipart_uploads
                 (upload_id, resource_id, account_id, object_key, storage_class,
                  http_metadata, custom_metadata, object_version, state, created_at_ms, updated_at_ms)
                 VALUES('upload', ?1, ?2, 'multipart', 'Standard', '{}', '{}', 'v3', 'initiating', 7, 7)",
                rusqlite::params![bucket, multipart_owner],
            )
            .unwrap();
        let version = open_compute_core::VersionId::generate().to_string();
        connection
            .execute(
                "INSERT INTO worker_versions
                 (id, worker_id, version_number, content_kind, state, worker_code_sha256,
                  loader_schema_version, compatibility_date, compatibility_flags_json,
                  created_at_ms, ready_at_ms)
                 VALUES(?1, ?2, 1, 'assets_only', 'ready', zeroblob(32), 1,
                        '2025-01-01', CAST('[]' AS BLOB), 7, 7)",
                rusqlite::params![version, worker],
            )
            .unwrap();
        let artifact_namespace = ResourceId::generate().to_string();
        let artifact_owner = if artifact_mismatch {
            connection
                .pragma_update(None, "foreign_keys", "OFF")
                .unwrap();
            InstanceId::generate().to_string()
        } else {
            id.clone()
        };
        connection
            .execute(
                "INSERT INTO artifact_namespaces
                 (id, account_id, name, max_repositories, max_tokens_per_repository,
                  created_at_ms, updated_at_ms)
                 VALUES(?1, ?2, 'artifacts', 100, 10, 7, 7)",
                rusqlite::params![artifact_namespace, artifact_owner],
            )
            .unwrap();
        if artifact_mismatch {
            connection
                .pragma_update(None, "foreign_keys", "ON")
                .unwrap();
        }
        let artifact_repository = open_compute_core::ArtifactRepoId::generate().to_string();
        connection
            .execute(
                "INSERT INTO artifact_repositories
                 (id, namespace_id, name, default_branch, state, created_at_ms, updated_at_ms)
                 VALUES(?1, ?2, 'repo', 'main', 'ready', 7, 7)",
                rusqlite::params![artifact_repository, artifact_namespace],
            )
            .unwrap();
        let artifact_token = open_compute_core::ArtifactTokenId::generate().to_string();
        connection
            .execute(
                "INSERT INTO artifact_repo_tokens
                 (id, repository_id, token_digest, scope, expires_at_ms, created_at_ms)
                 VALUES(?1, ?2, zeroblob(32), 'read', 1000, 7)",
                rusqlite::params![artifact_token, artifact_repository],
            )
            .unwrap();
        let artifact_binding = open_compute_core::BindingId::generate().to_string();
        connection
            .execute(
                "INSERT INTO version_artifact_bindings
                 (id, version_id, name, namespace_id, namespace_generation,
                  capability_version, permissions_json, descriptor_sha256, created_at_ms)
                 VALUES(?1, ?2, 'ARTIFACTS', ?3, 1, 1, CAST('{}' AS BLOB), zeroblob(32), 7)",
                rusqlite::params![artifact_binding, version, artifact_namespace],
            )
            .unwrap();
        let activation = open_compute_core::CronActivationId::generate().to_string();
        connection
            .execute(
                "INSERT INTO cron_activations
                 (id, account_id, worker_id, version_id, expression, expression_sha256,
                  parser_version, scheduled_handler, workflow_bindings_json,
                  activation_generation, state, availability, availability_code,
                  created_at_ms, updated_at_ms)
                 VALUES(?1, ?2, ?3, ?4, '* * * * *', zeroblob(32), 1, 1, CAST('[]' AS BLOB), 1,
                        'staging', 'degraded', 'CRON_PROJECTION_PENDING', 7, 7)",
                rusqlite::params![activation, id, worker, version],
            )
            .unwrap();
        let asset_owner = if asset_mismatch {
            connection
                .pragma_update(None, "foreign_keys", "OFF")
                .unwrap();
            InstanceId::generate().to_string()
        } else {
            id.clone()
        };
        connection
            .execute(
                "INSERT INTO asset_upload_sessions
                 (id, account_id, script_name, status, created_at_ms, expires_at_ms, updated_at_ms)
                 VALUES('asset-session', ?1, 'app', 'open', 7, 1000, 7)",
                [asset_owner],
            )
            .unwrap();
        if asset_mismatch {
            connection
                .pragma_update(None, "foreign_keys", "ON")
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO asset_upload_entries(session_id, path, wrangler_hash, size)
                 VALUES('asset-session', '/index.html', '0123456789abcdef0123456789abcdef', 4)",
                [],
            )
            .unwrap();
        let upload_owner = if version_upload_mismatch {
            connection
                .pragma_update(None, "foreign_keys", "OFF")
                .unwrap();
            InstanceId::generate().to_string()
        } else {
            id.clone()
        };
        connection
            .execute(
                "INSERT INTO version_uploads
                 (id, account_id, worker_id, idempotency_key, input_fingerprint,
                  content_kind, manifest_sha256, manifest_size, manifest_json,
                  routing_config_json, status, created_at_ms, expires_at_ms, updated_at_ms)
                 VALUES('version-session', ?1, ?2, 'upload-key', zeroblob(32),
                        'assets_only', zeroblob(32), 2, CAST('{}' AS BLOB),
                        CAST('{}' AS BLOB), 'open', 7, 1000, 7)",
                rusqlite::params![upload_owner, worker],
            )
            .unwrap();
        if version_upload_mismatch {
            connection
                .pragma_update(None, "foreign_keys", "ON")
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO version_upload_objects
                 (session_id, sha256, object_kind, size)
                 VALUES('version-session', zeroblob(32), 'asset_manifest', 2)",
                [],
            )
            .unwrap();
        let queue = open_compute_core::QueueId::generate().to_string();
        connection
            .execute(
                "INSERT INTO queues
                 (id, account_id, name, state, availability, lifecycle_generation,
                  config_generation, delivery_delay_seconds, retention_seconds,
                  max_message_bytes, max_batch_messages, max_batch_bytes,
                  max_backlog_bytes, created_at_ms, updated_at_ms)
                 VALUES(?1, ?2, 'jobs', 'ready', 'healthy', 1, 1, 0, 60,
                        1024, 10, 10240, 1048576, 7, 7)",
                rusqlite::params![queue, id],
            )
            .unwrap();
        let deployment = open_compute_core::DeploymentId::generate().to_string();
        connection
            .execute(
                "INSERT INTO worker_deployments
                 (id, worker_id, version_id, source, annotations_json, created_at_ms)
                 VALUES(?1, ?2, ?3, 'system', CAST('{}' AS BLOB), 7)",
                rusqlite::params![deployment, worker, version],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO deployment_runtime_assessments
                 (deployment_id, state, startup_id, updated_at_ms)
                 VALUES(?1, 'dispatchable', ?2, 7)",
                rusqlite::params![
                    deployment,
                    open_compute_core::StartupId::generate().to_string()
                ],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE workers SET active_deployment_id = ?1 WHERE id = ?2",
                rusqlite::params![deployment, worker],
            )
            .unwrap();
        let declaration = open_compute_core::QueueConsumerId::generate().to_string();
        connection
            .execute(
                "INSERT INTO version_queue_consumers
                 (id, version_id, origin, queue_id, queue_lifecycle_generation,
                  max_batch_size, max_batch_timeout_seconds, max_retries,
                  retry_delay_seconds, max_concurrency, capability_version,
                  descriptor_sha256, created_at_ms)
                 VALUES(?1, ?2, 'api', ?3, 1, 1, 0, 3, 0, 1, 1, zeroblob(32), 7)",
                rusqlite::params![declaration, version, queue],
            )
            .unwrap();
        let consumer = open_compute_core::QueueConsumerId::generate().to_string();
        connection
            .execute(
                "INSERT INTO queue_consumers
                 (id, account_id, queue_id, worker_id, declaration_id, version_id,
                  consumer_generation, state, availability, availability_code,
                  created_at_ms, updated_at_ms)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, 1, 'activating', 'degraded',
                        'QUEUE_CONSUMER_PROJECTION_PENDING', 7, 7)",
                rusqlite::params![consumer, id, queue, worker, declaration, version],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO worker_delete_intents(worker_id, account_id, request_id, created_at_ms)
                 VALUES(?1, ?2, 'recovery-request', 7)",
                rusqlite::params![worker, id],
            )
            .unwrap();
        let stored_id = if identity_mismatch {
            InstanceId::generate().to_string()
        } else {
            id.clone()
        };
        connection
            .execute(
                "INSERT INTO platform_meta(key, value, updated_at_ms)
                 VALUES('instance_id', CAST(?1 AS BLOB), 7),
                       ('created_at_ms', CAST('7' AS BLOB), 7)",
                [stored_id],
            )
            .unwrap();
        drop(connection);

        let db = ControlDb::open(&path, 100).unwrap();
        let result = apply(&db, &DeterministicClock::new(UNIX_EPOCH));
        if identity_mismatch
            || audit_mismatch
            || idempotency_mismatch
            || r2_mismatch
            || multipart_mismatch
            || asset_mismatch
            || version_upload_mismatch
            || resource_mismatch
            || artifact_mismatch
            || route_mismatch
        {
            assert_eq!(result.unwrap_err().code(), ErrorCode::MigrationFailed);
            assert!(db.table_exists("accounts").unwrap());
            assert!(!db.table_exists("instance_identity").unwrap());
            assert_eq!(inspect_schema(&db).unwrap(), 8);
        } else {
            result.unwrap();
            assert!(!db.table_exists("accounts").unwrap());
            assert!(db.table_exists("instance_identity").unwrap());
            assert_eq!(
                crate::CronRepository::new(&db)
                    .stage_activations(
                        InstanceId::generate(),
                        open_compute_core::WorkerId::generate(),
                        open_compute_core::VersionId::generate(),
                        1,
                        &[],
                        7,
                    )
                    .unwrap_err()
                    .code(),
                ErrorCode::InstanceNotFound,
            );
            db.with_read(|connection| {
                let (stored, created): (String, i64) = connection
                    .query_row(
                        "SELECT instance_id, created_at_ms FROM instance_identity",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .unwrap();
                assert_eq!((stored, created), (id, 7));
                let stored_worker: String = connection
                    .query_row("SELECT id FROM workers", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(stored_worker, worker);
                let route_worker: String = connection
                    .query_row("SELECT worker_id FROM worker_host_routes WHERE id = 'route'", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(route_worker, worker);
                let workflow_name: String = connection
                    .query_row(
                        "SELECT name FROM workflow_definitions WHERE id = ?1",
                        [&workflow],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(workflow_name, "flow");
                for table in [
                    "workers",
                    "hostname_claims",
                    "worker_host_routes",
                    "workflow_definitions",
                ] {
                    let count: i64 = connection
                        .query_row(
                            "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = 'account_id'",
                            [table],
                            |row| row.get(0),
                        )
                        .unwrap();
                    assert_eq!(count, 0, "{table} still has account_id");
                }
                let audit_action: String = connection
                    .query_row("SELECT action FROM control_audit_events WHERE seq = 1", [], |row| {
                        row.get(0)
                    })
                    .unwrap();
                assert_eq!(audit_action, "worker.create");
                let audit_account_columns: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM pragma_table_info('control_audit_events') WHERE name = 'account_id'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(audit_account_columns, 0);
                let response: Vec<u8> = connection
                    .query_row(
                        "SELECT response_json FROM control_idempotency
                         WHERE scope = 'worker.create' AND idempotency_key = 'key'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(response, b"{}");
                let idempotency_account_columns: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM pragma_table_info('control_idempotency') WHERE name = 'account_id'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(idempotency_account_columns, 0);
                let intent: (String, String) = connection
                    .query_row(
                        "SELECT worker_id, request_id FROM worker_delete_intents",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .unwrap();
                assert_eq!(intent.0, worker);
                assert_eq!(intent.1, "recovery-request");
                let intent_account_columns: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM pragma_table_info('worker_delete_intents') WHERE name = 'account_id'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(intent_account_columns, 0);
                let pin_worker: String = connection
                    .query_row(
                        "SELECT worker_id FROM system_owned_versions WHERE kind = 'dashboard'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(pin_worker, worker);
                let pin_account_columns: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM pragma_table_info('system_owned_versions') WHERE name = 'account_id'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(pin_account_columns, 0);
                for table in ["r2_objects", "r2_object_mutations", "r2_multipart_uploads"] {
                    let count: i64 = connection
                        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))
                        .unwrap();
                    assert_eq!(count, 1);
                    let account_columns: i64 = connection
                        .query_row(
                            &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = 'account_id'"),
                            [],
                            |row| row.get(0),
                        )
                        .unwrap();
                    assert_eq!(account_columns, 0);
                }
                let cron_account_columns: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM pragma_table_info('cron_activations') WHERE name = 'account_id'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(cron_account_columns, 0);
                let cron_rows: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM cron_activations WHERE id = ?1 AND version_id = ?2",
                        rusqlite::params![activation, version],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(cron_rows, 1);
                let cron_referrers: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM version_referrers
                         WHERE version_id = ?1 AND kind = 'cron_activation' AND ref_id = ?2",
                        rusqlite::params![version, activation],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(cron_referrers, 1);
                let asset_account_columns: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM pragma_table_info('asset_upload_sessions') WHERE name = 'account_id'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(asset_account_columns, 0);
                let asset_entries: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM asset_upload_entries WHERE session_id = 'asset-session'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(asset_entries, 1);
                let version_upload_account_columns: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM pragma_table_info('version_uploads') WHERE name = 'account_id'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(version_upload_account_columns, 0);
                let version_objects: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM version_upload_objects WHERE session_id = 'version-session'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(version_objects, 1);
                let consumer_account_columns: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM pragma_table_info('queue_consumers') WHERE name = 'account_id'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(consumer_account_columns, 0);
                let queue_account_columns: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM pragma_table_info('queues') WHERE name = 'account_id'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(queue_account_columns, 0);
                let preserved_queue: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM queues WHERE id = ?1 AND name = 'jobs'",
                        [&queue],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(preserved_queue, 1);
                let resource_account_columns: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM pragma_table_info('resources') WHERE name = 'account_id'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(resource_account_columns, 0);
                let preserved_resource: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM resources WHERE id = ?1 AND name = 'bucket'",
                        [&bucket],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(preserved_resource, 1);
                let namespace_account_columns: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM pragma_table_info('artifact_namespaces') WHERE name = 'account_id'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(namespace_account_columns, 0);
                for (table, key, value) in [
                    ("artifact_namespaces", "id", &artifact_namespace),
                    ("artifact_repositories", "id", &artifact_repository),
                    ("artifact_repo_tokens", "id", &artifact_token),
                    ("version_artifact_bindings", "id", &artifact_binding),
                ] {
                    let count: i64 = connection
                        .query_row(
                            &format!("SELECT COUNT(*) FROM {table} WHERE {key} = ?1"),
                            [value],
                            |row| row.get(0),
                        )
                        .unwrap();
                    assert_eq!(count, 1, "{table} row was not preserved");
                }
                let preserved_consumer: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM queue_consumers
                         WHERE id = ?1 AND queue_id = ?2 AND declaration_id = ?3",
                        rusqlite::params![consumer, queue, declaration],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(preserved_consumer, 1);
                assert_eq!(
                    connection
                        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| row.get::<_, i64>(0))
                        .unwrap(),
                    0,
                );
                assert_eq!(
                    connection
                        .query_row(
                            "SELECT COUNT(*) FROM platform_meta WHERE key IN ('instance_id', 'created_at_ms')",
                            [],
                            |row| row.get::<_, i64>(0),
                        )
                        .unwrap(),
                    0,
                );
                Ok(())
            })
            .unwrap();
        }
    }
}

#[test]
fn fresh_control_uses_only_complete_refinery_history() {
    let directory = tempfile::tempdir().unwrap();
    let db = ControlDb::open(&directory.path().join("control.sqlite"), 100).unwrap();
    apply(&db, &DeterministicClock::new(UNIX_EPOCH)).unwrap();
    assert_eq!(inspect_schema(&db).unwrap(), current_schema_version());
    assert!(!db.table_exists("schema_migrations").unwrap());
    assert!(db.table_exists("refinery_schema_history").unwrap());
    assert_eq!(db.user_version().unwrap(), 0);
}

#[test]
fn r2_size_migration_preserves_existing_objects_as_unknown() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("control.sqlite");
    let mut connection = Connection::open(&path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    schema_migrations::migrate_to_for_test(&mut connection, DatabaseKind::Control, 11);
    let instance = InstanceId::generate();
    let bucket = ResourceId::generate();
    connection
        .execute(
            "INSERT INTO instance_identity(instance_id, created_at_ms) VALUES (?1, 1)",
            [instance.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO resources(id, kind, name, state,
                                   driver_schema_version, created_at_ms, updated_at_ms)
             VALUES (?1, 'r2_bucket', 'bucket-one', 'creating', 1, 1, 1)",
            [bucket.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO r2_buckets(resource_id, physical_prefix, schema_version,
                                    max_object_bytes, object_authority_sha256, created_at_ms)
             VALUES (?1, ?2, 1, 1024, ?3, 1)",
            params![
                bucket.to_string(),
                format!("tenant/r2/v1/{bucket}/"),
                vec![1_u8; 32]
            ],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE resources SET state='ready' WHERE id=?1",
            [bucket.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO r2_objects(resource_id, object_key, object_version, updated_at_ms)
             VALUES (?1, 'old.txt', 'version-1', 1)",
            [bucket.to_string()],
        )
        .unwrap();
    drop(connection);

    let db = ControlDb::open(&path, 100).unwrap();
    apply(&db, &DeterministicClock::new(UNIX_EPOCH)).unwrap();
    assert_eq!(inspect_schema(&db).unwrap(), current_schema_version());
    let usage = crate::R2ObjectRepository::new(&db)
        .bucket_usage(instance, bucket)
        .unwrap();
    assert_eq!(usage.object_count, 1);
    assert_eq!(usage.size_bytes, None);
    db.with_immediate(|tx| {
        assert!(
            tx.execute(
                "UPDATE r2_objects SET size_bytes=-1 WHERE resource_id=?1",
                [bucket.to_string()],
            )
            .is_err()
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn untagged_version_metadata_migration_preserves_rows_and_guards() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("control.sqlite");
    let mut connection = Connection::open(&path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    schema_migrations::migrate_to_for_test(&mut connection, DatabaseKind::Control, 12);
    connection
        .execute_batch(
            "INSERT INTO instance_identity(instance_id, created_at_ms)
             VALUES('00000000-0000-7000-8000-000000000001', 1);
             INSERT INTO workers(
               id, name, active_deployment_id, do_storage_id,
               route_generation, created_at_ms, updated_at_ms, deleted_at_ms, ownership
             ) VALUES(
               '00000000-0000-7000-8000-000000000002',
               'worker', NULL,
               '00000000-0000-7000-8000-000000000003', 0, 1, 1, NULL, 'system'
             );
             INSERT INTO worker_versions(
               id, worker_id, version_number, content_kind, state,
               artifact_sha256, artifact_size, artifact_schema_version, main_module,
               worker_code_sha256, loader_schema_version, compatibility_date,
               compatibility_flags_json, created_at_ms
             ) VALUES(
               '00000000-0000-7000-8000-000000000004',
               '00000000-0000-7000-8000-000000000002', 1, 'worker', 'staging',
               zeroblob(32), 1, 1, 'index.js', zeroblob(32), 1, '2026-09-08', X'5B5D', 1
             );
             INSERT INTO version_builtin_bindings(
               version_id, binding_name, kind, tag, descriptor_sha256
             ) VALUES(
               '00000000-0000-7000-8000-000000000004', 'VERSION',
               'version_metadata', 'release', zeroblob(32)
             );
             UPDATE worker_versions SET state='validating'
             WHERE id='00000000-0000-7000-8000-000000000004';
             UPDATE worker_versions SET state='ready', ready_at_ms=2
             WHERE id='00000000-0000-7000-8000-000000000004';
             INSERT INTO worker_versions(
               id, worker_id, version_number, content_kind, state,
               artifact_sha256, artifact_size, artifact_schema_version, main_module,
               worker_code_sha256, loader_schema_version, compatibility_date,
               compatibility_flags_json, created_at_ms
             ) VALUES(
               '00000000-0000-7000-8000-000000000005',
               '00000000-0000-7000-8000-000000000002', 2, 'worker', 'staging',
               zeroblob(32), 1, 1, 'index.js', zeroblob(32), 1, '2026-09-08', X'5B5D', 2
             );",
        )
        .unwrap();
    assert!(
        connection
            .execute(
                "INSERT INTO version_builtin_bindings
             (version_id, binding_name, kind, tag, descriptor_sha256)
             VALUES (?1, 'UNTAGGED', 'version_metadata', NULL, zeroblob(32))",
                ["00000000-0000-7000-8000-000000000005"],
            )
            .is_err()
    );
    drop(connection);

    let db = ControlDb::open(&path, 100).unwrap();
    apply(&db, &DeterministicClock::new(UNIX_EPOCH)).unwrap();
    assert_eq!(inspect_schema(&db).unwrap(), current_schema_version());
    db.with_immediate(|tx| {
        let preserved: String = tx
            .query_row(
                "SELECT tag FROM version_builtin_bindings WHERE binding_name='VERSION'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| migration_failed())?;
        assert_eq!(preserved, "release");
        assert!(
            tx.execute(
                "DELETE FROM version_builtin_bindings WHERE binding_name='VERSION'",
                [],
            )
            .is_err()
        );
        tx.execute(
            "INSERT INTO version_builtin_bindings
             (version_id, binding_name, kind, tag, descriptor_sha256)
             VALUES (?1, 'UNTAGGED', 'version_metadata', NULL, zeroblob(32))",
            ["00000000-0000-7000-8000-000000000005"],
        )
        .map_err(|_| migration_failed())?;
        assert!(
            tx.execute(
                "INSERT INTO version_builtin_bindings
                 (version_id, binding_name, kind, tag, descriptor_sha256)
                 VALUES (?1, 'MODULE', 'wasm_module', NULL, zeroblob(32))",
                ["00000000-0000-7000-8000-000000000005"],
            )
            .is_err()
        );
        assert!(
            tx.execute(
                "UPDATE version_builtin_bindings SET tag='changed' WHERE binding_name='UNTAGGED'",
                [],
            )
            .is_err()
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn exact_empty_refinery_history_recovers_the_first_migration_crash_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let db = ControlDb::open(&directory.path().join("control.sqlite"), 100).unwrap();
    db.with_exclusive(|transaction| {
        transaction
            .execute_batch(
                "CREATE TABLE refinery_schema_history(
                   version int4 PRIMARY KEY,
                   name VARCHAR(255),
                   applied_on VARCHAR(255),
                   checksum VARCHAR(255)
                 );",
            )
            .map_err(|_| migration_failed())?;
        Ok(())
    })
    .unwrap();

    apply(&db, &DeterministicClock::new(UNIX_EPOCH)).unwrap();
    assert_eq!(inspect_schema(&db).unwrap(), current_schema_version());
}

#[test]
fn malformed_refinery_history_fails_closed() {
    let directory = tempfile::tempdir().unwrap();
    let db = ControlDb::open(&directory.path().join("control.sqlite"), 100).unwrap();
    apply(&db, &DeterministicClock::new(UNIX_EPOCH)).unwrap();
    db.with_exclusive(|transaction| {
        transaction
            .execute(
                "UPDATE refinery_schema_history SET checksum='not-a-checksum' WHERE version=1",
                [],
            )
            .map_err(|_| migration_failed())?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        inspect_schema(&db).unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
}

#[test]
fn refinery_history_table_definition_is_part_of_the_schema_head() {
    let directory = tempfile::tempdir().unwrap();
    let db = ControlDb::open(&directory.path().join("control.sqlite"), 100).unwrap();
    apply(&db, &DeterministicClock::new(UNIX_EPOCH)).unwrap();
    db.with_exclusive(|transaction| {
        transaction
            .execute_batch(
                "ALTER TABLE refinery_schema_history RENAME TO old_refinery_schema_history;
                 CREATE TABLE refinery_schema_history(
                   version INTEGER PRIMARY KEY,
                   name TEXT,
                   applied_on TEXT,
                   checksum TEXT
                 );
                 INSERT INTO refinery_schema_history
                 SELECT * FROM old_refinery_schema_history;
                 DROP TABLE old_refinery_schema_history;",
            )
            .map_err(|_| migration_failed())?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        inspect_schema(&db).unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
}

#[test]
fn refinery_history_cannot_mask_current_schema_drift() {
    let directory = tempfile::tempdir().unwrap();
    let db = ControlDb::open(&directory.path().join("control.sqlite"), 100).unwrap();
    apply(&db, &DeterministicClock::new(UNIX_EPOCH)).unwrap();
    db.with_exclusive(|transaction| {
        transaction
            .execute_batch("CREATE TABLE unexpected_platform_table(id INTEGER PRIMARY KEY) STRICT;")
            .map_err(|_| migration_failed())?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        inspect_schema(&db).unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
}

#[test]
fn partial_refinery_head_drift_is_rejected_before_the_next_migration() {
    if current_schema_version() < 2 {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let db = ControlDb::open(&directory.path().join("control.sqlite"), 100).unwrap();
    apply(&db, &DeterministicClock::new(UNIX_EPOCH)).unwrap();
    db.with_exclusive(|transaction| {
        transaction
            .execute_batch(
                "DELETE FROM refinery_schema_history WHERE version>=2;
                 ALTER TABLE worker_versions DROP COLUMN resource_limits_json;
                 CREATE TABLE unexpected_platform_table(id INTEGER PRIMARY KEY) STRICT;",
            )
            .map_err(|_| migration_failed())?;
        Ok(())
    })
    .unwrap();

    assert_eq!(
        apply(&db, &DeterministicClock::new(UNIX_EPOCH))
            .unwrap_err()
            .code(),
        ErrorCode::MigrationFailed
    );
    db.with_read(|connection| {
        let history: i64 = connection
            .query_row("SELECT COUNT(*) FROM refinery_schema_history", [], |row| {
                row.get(0)
            })
            .map_err(|_| migration_failed())?;
        let resource_limit_column: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('worker_versions')
                 WHERE name='resource_limits_json'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| migration_failed())?;
        assert_eq!((history, resource_limit_column), (1, 0));
        Ok(())
    })
    .unwrap();
}

#[test]
fn v1_schema_without_history_is_rejected_without_mutation() {
    let directory = tempfile::tempdir().unwrap();
    let db = ControlDb::open(&directory.path().join("control.sqlite"), 100).unwrap();
    db.with_exclusive(|transaction| {
        transaction
            .execute_batch(include_str!("../refinery-migrations/control/V1__init.sql"))
            .map_err(|_| migration_failed())?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        apply(&db, &DeterministicClock::new(UNIX_EPOCH))
            .unwrap_err()
            .code(),
        ErrorCode::MigrationFailed
    );
    assert!(!db.table_exists("refinery_schema_history").unwrap());
    assert!(!db.table_exists("schema_migrations").unwrap());
}

#[test]
fn local_extension_migration_rejects_old_service_descriptors_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("control.sqlite");
    let mut connection = Connection::open(&path).unwrap();
    schema_migrations::migrate_to_for_test(&mut connection, DatabaseKind::Control, 6);
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    connection
        .execute_batch(
            "INSERT INTO accounts(id, name, created_at_ms, deleted_at_ms)
             VALUES('00000000-0000-7000-8000-000000000001', 'account', 1, NULL);
             INSERT INTO workers(
               id, account_id, name, active_deployment_id, do_storage_id,
               route_generation, created_at_ms, updated_at_ms, deleted_at_ms, ownership
             ) VALUES(
               '00000000-0000-7000-8000-000000000002',
               '00000000-0000-7000-8000-000000000001', 'worker', NULL,
               '00000000-0000-7000-8000-000000000003', 0, 1, 1, NULL, 'tenant'
             );
             INSERT INTO worker_versions(
               id, worker_id, version_number, content_kind, state,
               artifact_sha256, artifact_size, artifact_schema_version, main_module,
               worker_code_sha256, loader_schema_version, compatibility_date,
               compatibility_flags_json, created_at_ms
             ) VALUES(
               '00000000-0000-7000-8000-000000000004',
               '00000000-0000-7000-8000-000000000002', 1, 'worker', 'staging',
               zeroblob(32), 1, 1, 'index.js', zeroblob(32), 1, '2026-09-08', X'5B5D', 1
             );
             INSERT INTO version_services(
               version_id, binding_name, target_worker_id, entrypoint,
               props_json, descriptor_sha256, created_at_ms
             ) VALUES(
               '00000000-0000-7000-8000-000000000004', 'SERVICE',
               '00000000-0000-7000-8000-000000000002', NULL, NULL, zeroblob(32), 1
             );",
        )
        .unwrap();
    drop(connection);

    let db = ControlDb::open(&path, 100).unwrap();
    assert_eq!(
        apply(&db, &DeterministicClock::new(UNIX_EPOCH))
            .unwrap_err()
            .code(),
        ErrorCode::MigrationFailed
    );
    db.with_read(|connection| {
        let state: (i64, i64, i64) = connection
            .query_row(
                "SELECT
                   (SELECT MAX(version) FROM refinery_schema_history),
                   (SELECT COUNT(*) FROM version_services),
                   (SELECT COUNT(*) FROM pragma_table_info('version_services')
                    WHERE name='target_kind')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|_| migration_failed())?;
        assert_eq!(state, (6, 1, 0));
        Ok(())
    })
    .unwrap();
}
