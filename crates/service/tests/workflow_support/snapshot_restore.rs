//! Real runtime replay after the authenticated P1 two-database snapshot path.

use super::*;
use open_compute_core::{HardeningConfig, PlatformConfig, StorageConfig};
use open_compute_service::capabilities::platform_capabilities;
use open_compute_storage::{
    PlatformStorage, PreparePlatformSnapshotRequest, RestoreTarget, inspect_master_key,
    prepare_platform_snapshot, sign_snapshot_manifest, verify_snapshot_manifest_mac,
};
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

#[path = "durable_snapshot.rs"]
mod durable;

fn storage_config(data: &Path) -> StorageConfig {
    StorageConfig {
        data_dir: data.to_owned(),
        master_key_file: data.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_snapshot_fresh_host_replays_committed_steps_with_fresh_generation() {
    let mut original = Harness::start().await;
    let config = WorkflowsConfig::default();
    let store = Arc::new(
        SchedulerStore::open(
            &original.storage.data_dir().ensure_scheduler_db().unwrap(),
            5000,
            now(),
        )
        .unwrap(),
    );
    let backend = start_backend(&mut original, &store, &config);
    let account = original.storage.identity().default_account_id;
    let definition = WorkflowRepository::new(original.storage.db())
        .create_definition(account, "snapshot-flow", now())
        .unwrap();
    let target = original.deploy(SOURCE, "Flow").await;
    let version = WorkflowApiState::new(
        original.storage.clone(),
        store.clone(),
        original.transport.clone(),
        Default::default(),
    )
    .create_version(account, definition.id, target.deployment_id, "Flow".into())
    .await
    .unwrap();
    let controller = WorkflowController::new(&original.storage, &store, &config);
    let identity = controller
        .create(
            account,
            definition.id,
            open_compute_core::WorkflowOperationId::generate(),
            Some("snapshot-instance"),
            open_compute_workers::WorkflowCreateInput {
                payload_base64: &encode_workflow_json(&serde_json::json!({"value":42})),
                retention: None,
                schedule: None,
            },
            now(),
        )
        .unwrap();
    assert_eq!(identity.external_instance_id, "snapshot-instance");
    let run = controller
        .claim(now(), &mut Default::default())
        .unwrap()
        .unwrap();
    let request = WorkflowRunRequest {
        fence: run.fence.clone(),
        external_instance_id: run.external_instance_id,
        definition_name: run.target.definition_name,
        created_at_ms: run.created_at_ms,
        payload_base64: run.input_json,
        rollback: run.rollback,
        schedule: None,
    };
    let result = original
        .transport
        .dispatch_workflow(
            &version.target,
            &request,
            Duration::from_millis(config.dispatch_timeout_ms),
        )
        .await
        .unwrap();
    let (_, before_output) = complete(result);
    let before = decode_workflow_json(&before_output);
    assert_eq!(before["callbacks"], 1.0);
    assert_eq!(
        store
            .workflow_instance(identity.instance_id)
            .unwrap()
            .unwrap()
            .completed_step_count,
        1
    );
    let durable = durable::prepare(&original.storage, &store, &config, target.deployment_id);
    // Lose only the terminal observation: no callback result is lost. Maintenance stops
    // all producers/runtime I/O before the two standalone database copies are prepared.
    original.quiesce().await;
    backend.await.unwrap().unwrap();
    for path in [
        original.storage.data_dir().control_db_path(),
        original.storage.data_dir().scheduler_db_path(),
    ] {
        let connection = rusqlite::Connection::open(path).unwrap();
        let busy = connection
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        assert_eq!(busy, 0);
    }
    let source_config = storage_config(original.storage.data_dir().root());
    let key = inspect_master_key(&source_config).unwrap();
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let release = platform_capabilities(&PlatformConfig::default())
        .unwrap()
        .release;
    let snapshot_id = RequestId::generate().to_string();
    let prefix = format!(
        "system/snapshots/v1/{}/{snapshot_id}/objects/",
        original.storage.identity().platform_id
    );
    let digest = "1".repeat(64);
    let mut snapshot = prepare_platform_snapshot(
        original.storage.data_dir(),
        &PreparePlatformSnapshotRequest {
            snapshot_id: &snapshot_id,
            label: "workflow-replay",
            created_at_ms: now(),
            release,
            master_key_fingerprint: key.fingerprint(),
            s3_authority_fingerprint: &digest,
            r2_prefix_fingerprint: &digest,
            config_policy_sha256: &digest,
            object_prefix: &prefix,
            hardening: &HardeningConfig::default(),
            sqlite_busy_timeout_ms: 5000,
        },
    )
    .unwrap();
    sign_snapshot_manifest(&mut snapshot.manifest, &key).unwrap();
    verify_snapshot_manifest_mac(&snapshot.manifest, &key).unwrap();
    assert_eq!(
        snapshot.manifest.source_schemas["control"],
        u32::try_from(open_compute_storage::migrations::current_schema_version()).unwrap()
    );
    assert_eq!(
        snapshot.manifest.source_schemas["scheduler"],
        u32::try_from(open_compute_storage::current_scheduler_schema_version()).unwrap()
    );
    let fresh = tempfile::Builder::new()
        .prefix("workflow-restored-")
        .tempdir_in(workspace.join(".temp/workflow-run"))
        .unwrap();
    let data = fresh.path().join("fresh-host");
    let restore = RestoreTarget::acquire(&data).unwrap();
    for file in &snapshot.files {
        let path = restore.destination_for(&file.entry.restore_path).unwrap();
        std::fs::copy(&file.staging_path, &path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    restore
        .validate_and_publish(&snapshot.manifest, key.fingerprint(), 5000, b"{}")
        .unwrap();
    let recovery_key = fresh.path().join("recovery.key");
    std::fs::copy(&source_config.master_key_file, &recovery_key).unwrap();
    std::fs::set_permissions(&recovery_key, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut config_storage = storage_config(&data);
    config_storage.master_key_file = recovery_key;
    let restored_storage =
        Arc::new(PlatformStorage::bootstrap(&config_storage, &SystemClock).unwrap());
    assert_eq!(
        restored_storage.identity().platform_id,
        original.storage.identity().platform_id
    );
    let restored_store =
        Arc::new(SchedulerStore::open(&data.join("scheduler.sqlite"), 5000, now()).unwrap());
    restored_store
        .verify_workflow_history(identity.instance_id)
        .unwrap();
    let restored_controller = WorkflowController::new(&restored_storage, &restored_store, &config);
    let expired_at = now() + i64::try_from(config.lease_ms + 1).unwrap();
    durable::verify(
        &restored_storage,
        &restored_store,
        &config,
        &durable,
        expired_at,
    );
    restored_controller
        .reconcile(&mut WorkflowReconcileCursor::default(), 32, expired_at)
        .unwrap();
    let replay = restored_controller
        .claim(
            expired_at + i64::try_from(config.recovery_backoff_ms).unwrap(),
            &mut Default::default(),
        )
        .unwrap()
        .unwrap();
    assert_ne!(replay.fence.run_token, run.fence.run_token);
    assert_eq!(
        restored_store
            .finish_workflow(
                &run.fence,
                &WorkflowCompletion::Complete {
                    output_json: "null".into(),
                    final_ordinal: 1
                },
                expired_at,
                &config
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowRunStale
    );
    let mut restored = Harness::boot(
        restored_storage.clone(),
        original.artifacts.clone(),
        original.mock.clone(),
        fresh,
    )
    .await;
    let restored_backend = start_backend(&mut restored, &restored_store, &config);
    let replay_request = WorkflowRunRequest {
        fence: replay.fence.clone(),
        external_instance_id: replay.external_instance_id,
        definition_name: replay.target.definition_name,
        created_at_ms: replay.created_at_ms,
        payload_base64: replay.input_json,
        rollback: replay.rollback,
        schedule: None,
    };
    let result = restored
        .transport
        .dispatch_workflow(
            &version.target,
            &replay_request,
            Duration::from_millis(config.dispatch_timeout_ms),
        )
        .await
        .unwrap();
    assert_eq!(result.loader_outcome, "cold");
    let (final_ordinal, output_base64) = complete(result);
    let after = decode_workflow_json(&output_base64);
    assert_eq!(after["callbacks"], 0.0);
    assert_eq!(after["value"], before["value"]);
    restored_store
        .finish_workflow(
            &replay.fence,
            &WorkflowCompletion::Complete {
                output_json: output_base64,
                final_ordinal,
            },
            expired_at + 1001,
            &config,
        )
        .unwrap();
    restored_controller
        .reconcile(
            &mut WorkflowReconcileCursor::default(),
            32,
            expired_at + 1002,
        )
        .unwrap();
    assert!(matches!(
        restored_controller
            .status(account, definition.id, identity.instance_id, 0)
            .unwrap(),
        open_compute_workers::WorkflowStatus::Complete { .. }
    ));
    restored.stop().await;
    restored_backend.await.unwrap().unwrap();
    original.stop().await;
}

const SOURCE: &str = r#"
import { WorkflowEntrypoint } from 'cloudflare:workers';
export class Flow extends WorkflowEntrypoint {
  async run(event, step) {
    let callbacks = 0;
    const value = await step.do('persist', () => { callbacks++; return {input:event.payload.value,nonce:crypto.randomUUID()}; });
    return {value,callbacks};
  }
}
export default { fetch() { return new Response('workflow'); } };
"#;
