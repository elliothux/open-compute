use super::*;

#[test]
fn authority_checks_active_version_and_resolves_current_route_epoch() {
    let (_temp, storage) = storage();
    let fixture = ready_fixture(&storage);
    let repo = DurableObjectRepository::new(&storage);
    let wrong = public_id(ResourceId::generate(), 4);
    assert_eq!(
        repo.authorize_dispatch(
            fixture.binding,
            fixture.version,
            &fixture.descriptor,
            wrong,
            20,
            true,
        )
        .unwrap_err()
        .code(),
        ErrorCode::DoIdInvalid
    );
    let object = public_id(fixture.namespace, 5);
    let workers = WorkerRepository::new(storage.db());
    let advanced = workers
        .promote(
            fixture.account,
            fixture.worker,
            fixture.version,
            None,
            RequestId::generate(),
            21,
        )
        .unwrap();
    assert!(advanced.route_generation > fixture.route_generation);
    let dispatch = repo
        .authorize_dispatch(
            fixture.binding,
            fixture.version,
            &fixture.descriptor,
            object,
            22,
            true,
        )
        .unwrap();
    assert_eq!(dispatch.route_generation, advanced.route_generation);
    let replacement = VersionId::generate();
    workers
        .insert_staging_version(
            &NewVersion {
                id: replacement,
                account_id: fixture.account,
                worker_id: fixture.worker,
                content_kind: crate::VersionContentKind::Worker,
                artifact_sha256: Some([1; 32]),
                artifact_size: Some(1),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".to_owned()),
                worker_code_sha256: [10; 32],
                compatibility_date: "2026-08-30".into(),
                compatibility_flags: Vec::new(),
                vars: BTreeMap::new(),
                secrets: BTreeMap::new(),
                request_id: RequestId::generate(),
                now_ms: 23,
            },
            &crate::NewVersionProducts::default(),
            1_000_000,
        )
        .unwrap();
    workers.begin_validation(replacement).unwrap();
    workers.mark_ready(replacement, 24).unwrap();
    workers
        .promote(
            fixture.account,
            fixture.worker,
            replacement,
            None,
            RequestId::generate(),
            25,
        )
        .unwrap();
    assert_eq!(
        repo.authorize_dispatch(
            fixture.binding,
            fixture.version,
            &fixture.descriptor,
            object,
            26,
            true
        )
        .unwrap_err()
        .code(),
        ErrorCode::DoVersionStale
    );
    let rollback = workers
        .promote(
            fixture.account,
            fixture.worker,
            fixture.version,
            None,
            RequestId::generate(),
            27,
        )
        .unwrap();
    let resumed = repo
        .authorize_dispatch(
            fixture.binding,
            fixture.version,
            &fixture.descriptor,
            object,
            28,
            true,
        )
        .unwrap();
    assert_eq!(resumed.route_generation, rollback.route_generation);
    assert!(resumed.route_generation > dispatch.route_generation);
    assert_eq!(resumed.object_generation, dispatch.object_generation);
    assert_eq!(resumed.host_key, dispatch.host_key);
    assert!(repo.has_live_objects(fixture.namespace).unwrap());
    assert_eq!(
        repo.list_objects(fixture.account, fixture.namespace)
            .unwrap()
            .len(),
        1
    );
}
