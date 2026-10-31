use super::*;
use crate::worker_repository::{NewVersionProducts, VersionContentKind};
use open_compute_core::RequestId;

#[test]
fn upgrade_preflight_rejects_unsupported_loader_schema_without_mutation() {
    let (_temp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let account = storage.identity().instance_id;
    let repository = WorkerRepository::new(storage.db());
    let worker = repository
        .create_worker(account, "upgrade-boundary", RequestId::generate(), 1, 100)
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
            &NewVersionProducts::default(),
            100,
        )
        .unwrap();
    let path = root.join("control.sqlite");
    crate::control_db::ControlDb::preflight_migrations(&path, 5_000, &SystemClock).unwrap();
    storage
        .db()
        .with_connection_mut(|connection| {
            connection
                .execute(
                    "UPDATE worker_versions SET loader_schema_version=1 WHERE id=?1",
                    [version],
                )
                .unwrap();
            Ok(())
        })
        .unwrap();
    repository.begin_validation(version).unwrap();
    repository.mark_ready(version, 3).unwrap();
    drop(storage);
    let before = fs::read(&path).unwrap();
    assert_eq!(
        crate::control_db::ControlDb::preflight_migrations(&path, 5_000, &SystemClock)
            .unwrap_err()
            .code(),
        ErrorCode::MigrationFailed
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT loader_schema_version FROM worker_versions WHERE id=?1",
                [version],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}
