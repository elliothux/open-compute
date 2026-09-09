use super::*;

#[test]
fn snapshot_version_artifact_inventory_uses_the_canonical_sharded_key() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let request = open_compute_core::RequestId::generate();
    let (worker, _) = repo
        .create_worker(account, "snapshot-key", request, 1, 1_000_000)
        .unwrap();
    repo.insert_staging_version(
        &NewVersion {
            id: VersionId::generate(),
            account_id: account,
            worker_id: worker.id,
            content_kind: crate::VersionContentKind::Worker,
            artifact_sha256: Some([1; 32]),
            artifact_size: Some(123),
            artifact_schema_version: Some(1),
            main_module: Some("index.js".to_owned()),
            worker_code_sha256: [2; 32],
            compatibility_date: "2026-09-08".into(),
            compatibility_flags: Vec::new(),
            vars: BTreeMap::new(),
            secrets: BTreeMap::new(),
            request_id: request,
            now_ms: 2,
        },
        &crate::NewVersionProducts::default(),
        1_000_000,
    )
    .unwrap();
    drop(storage);

    let references = crate::inspect_snapshot_immutable_references(
        &root.join("control.sqlite"),
        5_000,
        "system/",
    )
    .unwrap();
    assert_eq!(references.len(), 1);
    assert_eq!(references[0].role, "version_artifact");
    assert_eq!(
        references[0].object_key,
        format!("system/artifacts/v1/sha256/01/{}", "01".repeat(31))
    );
}
