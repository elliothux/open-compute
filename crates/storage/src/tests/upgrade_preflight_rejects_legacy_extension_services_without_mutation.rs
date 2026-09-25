use super::*;
use crate::{NewVersionProducts, NewVersionService, ServiceTarget, VersionContentKind};
use open_compute_core::RequestId;

#[test]
fn upgrade_preflight_rejects_legacy_extension_services_without_mutation() {
    let (_temp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let account = storage.identity().instance_id;
    let repository = WorkerRepository::new(storage.db());
    let worker = repository
        .create_worker(account, "caller", RequestId::generate(), 1, 100)
        .unwrap()
        .0;
    let version = VersionId::generate();
    repository
        .insert_staging_version(
            &NewVersion {
                id: version,
                instance_id: account,
                worker_id: worker.id,
                content_kind: VersionContentKind::Worker,
                artifact_sha256: Some([1; 32]),
                artifact_size: Some(1),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".to_owned()),
                worker_code_sha256: [2; 32],
                compatibility_date: "2026-09-08".to_owned(),
                compatibility_flags: Vec::new(),
                resource_limits: EffectiveResourceLimits::standard_defaults(),
                vars: BTreeMap::new(),
                secrets: BTreeMap::new(),
                request_id: RequestId::generate(),
                now_ms: 2,
            },
            &NewVersionProducts {
                services: &[NewVersionService {
                    binding_name: "PRIVATE".to_owned(),
                    target: ServiceTarget::Extension {
                        name: "private-api".to_owned(),
                        policy_revision: "a".repeat(64),
                    },
                    entrypoint: None,
                    props_json: None,
                    descriptor_sha256: [3; 32],
                }],
                ..Default::default()
            },
            100,
        )
        .unwrap();
    drop(storage);

    let path = root.join("control.sqlite");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "ALTER TABLE version_services DROP COLUMN target_policy_revision;
             DELETE FROM refinery_schema_history WHERE version = 11;",
        )
        .unwrap();
    drop(connection);

    assert_eq!(
        crate::ControlDb::preflight_migrations(&path, 5_000, &SystemClock)
            .unwrap_err()
            .code(),
        ErrorCode::MigrationFailed
    );
    let connection = Connection::open(&path).unwrap();
    let columns: Vec<String> = connection
        .prepare("PRAGMA table_info(version_services)")
        .unwrap()
        .query_map([], |row| row.get(1))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        !columns
            .iter()
            .any(|column| column == "target_policy_revision")
    );
}
