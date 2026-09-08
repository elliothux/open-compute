use super::*;

#[test]
fn p0_2_repository_enforces_lifecycle_immutability_and_idempotency() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let request = open_compute_core::RequestId::generate();
    let (worker, route) = repo
        .create_worker(account, "hello-worker", request, 1_000, 1_000_000)
        .unwrap();
    assert_eq!(
        route.path_prefix,
        format!("/__workers/{account}/hello-worker/")
    );
    assert_eq!(
        repo.create_worker(account, "hello-worker", request, 1_001, 1_000_000)
            .unwrap_err()
            .code(),
        ErrorCode::WorkerNameConflict
    );

    let version = VersionId::generate();
    let revision = uuid::Uuid::now_v7().to_string();
    let envelope = storage
        .crypto()
        .encrypt(
            &SecretBytes::new(b"never-persist-plaintext".to_vec()),
            account,
            worker.id,
            version,
            "API_TOKEN",
            &revision,
        )
        .unwrap();
    let mut vars = BTreeMap::new();
    vars.insert("MODE".to_owned(), br#""production""#.to_vec());
    let mut secrets = BTreeMap::new();
    secrets.insert(
        "API_TOKEN".to_owned(),
        StoredVersionSecret {
            name: "API_TOKEN".to_owned(),
            revision_id: revision.clone(),
            envelope,
        },
    );
    let created = repo
        .insert_staging_version(
            &NewVersion {
                id: version,
                account_id: account,
                worker_id: worker.id,
                content_kind: crate::VersionContentKind::Worker,
                artifact_sha256: Some([1; 32]),
                artifact_size: Some(123),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".to_owned()),
                worker_code_sha256: [2; 32],
                compatibility_date: "2026-08-30".into(),
                compatibility_flags: Vec::new(),
                vars,
                secrets,
                request_id: request,
                now_ms: 2_000,
            },
            &crate::NewVersionProducts::default(),
            1_000_000,
        )
        .unwrap();
    assert_eq!(created.version_number, 1);
    assert_eq!(created.state, VersionState::Staging);
    repo.begin_validation(version).unwrap();
    repo.mark_ready(version, 2_100).unwrap();
    let promoted = repo
        .promote(account, worker.id, version, None, request, 2_200)
        .unwrap();
    assert_eq!(promoted.active_version_id, Some(version));
    let resolved = repo
        .resolve_route(None, &format!("{}path", route.path_prefix))
        .unwrap();
    assert_eq!(resolved.version.id, version);
    let snapshot = repo
        .version_snapshot(account, worker.id, version, false)
        .unwrap();
    let secret = snapshot.secrets.get("API_TOKEN").unwrap();
    let plaintext = storage
        .crypto()
        .decrypt(
            &secret.envelope,
            account,
            worker.id,
            version,
            "API_TOKEN",
            &revision,
        )
        .unwrap();
    assert_eq!(plaintext.expose(), b"never-persist-plaintext");

    let db_path = root.join("control.sqlite");
    let conn = Connection::open(&db_path).unwrap();
    assert!(
        conn.execute(
            "UPDATE worker_versions SET main_module = 'changed.js' WHERE id = ?1",
            [version.to_string()],
        )
        .is_err()
    );
    drop(conn);
    assert_eq!(
        repo.tombstone_version(account, worker.id, version, request, 3_000)
            .unwrap_err()
            .code(),
        ErrorCode::VersionActive
    );

    let fingerprint = storage.crypto().fingerprint_request(b"canonical request");
    assert_eq!(
        repo.reserve_idempotency(
            account,
            "worker.create",
            "key-1",
            storage.crypto().fingerprint_key_id(),
            &fingerprint,
            4_000,
            5_000,
        )
        .unwrap(),
        IdempotencyReservation::Reserved
    );
    repo.complete_idempotency(
        account,
        "worker.create",
        "key-1",
        &fingerprint,
        br#"{"ok":true}"#,
    )
    .unwrap();
    assert_eq!(
        repo.reserve_idempotency(
            account,
            "worker.create",
            "key-1",
            storage.crypto().fingerprint_key_id(),
            &fingerprint,
            4_001,
            5_001,
        )
        .unwrap(),
        IdempotencyReservation::Complete(br#"{"ok":true}"#.to_vec())
    );
    let other = storage.crypto().fingerprint_request(b"different request");
    assert_eq!(
        repo.reserve_idempotency(
            account,
            "worker.create",
            "key-1",
            storage.crypto().fingerprint_key_id(),
            &other,
            4_002,
            5_002,
        )
        .unwrap_err()
        .code(),
        ErrorCode::IdempotencyConflict
    );

    let raw = fs::read(db_path).unwrap();
    assert!(
        !raw.windows(b"never-persist-plaintext".len())
            .any(|window| window == b"never-persist-plaintext")
    );
}
