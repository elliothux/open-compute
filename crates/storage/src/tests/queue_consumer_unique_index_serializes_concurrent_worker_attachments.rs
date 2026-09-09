use super::*;

#[test]
fn queue_consumer_unique_index_serializes_concurrent_worker_attachments() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let queue_id = open_compute_core::QueueId::generate();
    let queue_config = crate::QueueConfig::default();
    let queues = crate::QueueRepository::new(storage.db());
    queues
        .insert_creating(account, queue_id, "one-consumer", queue_config, 1)
        .unwrap();
    queues.mark_ready(account, queue_id, 2).unwrap();

    let workers = WorkerRepository::new(storage.db());
    let request = open_compute_core::RequestId::generate();
    let (first_worker, _) = workers
        .create_worker(account, "consumer-race-a", request, 3, 1_000_000)
        .unwrap();
    let (second_worker, _) = workers
        .create_worker(account, "consumer-race-b", request, 4, 1_000_000)
        .unwrap();
    let create_declaration = |worker_id: WorkerId, now_ms: i64| {
        let version_id = VersionId::generate();
        let declaration_id = QueueConsumerId::generate();
        workers
            .insert_staging_version(
                &NewVersion {
                    id: version_id,
                    account_id: account,
                    worker_id,
                    content_kind: crate::VersionContentKind::Worker,
                    artifact_sha256: Some([5; 32]),
                    artifact_size: Some(100),
                    artifact_schema_version: Some(1),
                    main_module: Some("index.js".to_owned()),
                    worker_code_sha256: [6; 32],
                    compatibility_date: "2026-09-08".into(),
                    compatibility_flags: Vec::new(),
                    vars: BTreeMap::new(),
                    secrets: BTreeMap::new(),
                    request_id: request,
                    now_ms,
                },
                &crate::NewVersionProducts {
                    queue_consumers: &[NewQueueConsumerDeclaration {
                        id: declaration_id,
                        queue_id,
                        queue_lifecycle_generation: 1,
                        entrypoint: None,
                        config: QueueConsumerConfig::default(),
                        dead_letter_queue: None,
                        capability_version: 1,
                        descriptor_sha256: [7; 32],
                    }],
                    ..Default::default()
                },
                10,
            )
            .unwrap();
        workers.begin_validation(version_id).unwrap();
        workers.mark_ready(version_id, now_ms + 1).unwrap();
        QueueConsumerRepository::new(storage.db())
            .declaration(declaration_id)
            .unwrap()
    };
    let first = create_declaration(first_worker.id, 10);
    let second = create_declaration(second_worker.id, 20);
    let first_db = crate::ControlDb::open(&root.join("control.sqlite"), 5_000).unwrap();
    let second_db = crate::ControlDb::open(&root.join("control.sqlite"), 5_000).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let results = thread::scope(|scope| {
        let first_barrier = barrier.clone();
        let first_handle = scope.spawn(move || {
            first_barrier.wait();
            QueueConsumerRepository::new(&first_db).create_attachment(
                account,
                first_worker.id,
                &first,
                30,
            )
        });
        let second_barrier = barrier.clone();
        let second_handle = scope.spawn(move || {
            second_barrier.wait();
            QueueConsumerRepository::new(&second_db).create_attachment(
                account,
                second_worker.id,
                &second,
                30,
            )
        });
        [first_handle.join().unwrap(), second_handle.join().unwrap()]
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let failure = results
        .iter()
        .find_map(|result| result.as_ref().err())
        .unwrap();
    assert_eq!(failure.code(), ErrorCode::QueueConsumerConflict);
    assert_eq!(
        QueueConsumerRepository::new(storage.db())
            .list_live(10)
            .unwrap()
            .len(),
        1
    );
}
