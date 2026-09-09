use super::*;

pub(super) async fn reject_missing_class(case: &MatrixCase<'_>) {
    let _transport = case.transport;
    let _supervisor = case.supervisor;
    let _storage = case.storage;
    let account = case.account;
    let worker_id = case.worker.id;
    let counter = case.counter;
    let other = case.other;
    let output_queue_resource = case.output_queue_resource;
    let _version_a = case.version_a;
    let _generation_a = case.generation_a;
    let versions = case.versions;
    let mut missing_class = version_request(
        account,
        worker_id,
        counter,
        other,
        output_queue_resource,
        "missing-class",
        "invalid",
        29,
        false,
    );
    missing_class.content = VersionContent::Worker {
        bundle: CanonicalBundle::build(
            "index.js",
            vec![ModuleInput {
                name: "index.js".to_owned(),
                module_type: ModuleType::EsModule,
                bytes: b"export default { fetch() { return new Response('missing'); } };".to_vec(),
            }],
            BundleLimits::default(),
        )
        .unwrap()
        .into_bytes()
        .into(),
        assets: None,
    };
    assert_eq!(
        versions
            .create_version(missing_class)
            .await
            .unwrap_err()
            .code(),
        open_compute_core::ErrorCode::DoClassNotFound
    );
}

pub(super) async fn promote(case: &MatrixCase<'_>) -> (VersionRecord, u64) {
    let transport = case.transport;
    let supervisor = case.supervisor;
    let storage = case.storage;
    let account = case.account;
    let worker_id = case.worker.id;
    let counter = case.counter;
    let other = case.other;
    let output_queue_resource = case.output_queue_resource;
    let version_a = case.version_a;
    let generation_a = case.generation_a;
    let versions = case.versions;
    let workers = WorkerRepository::new(storage.db());
    let in_flight = tokio::spawn({
        let transport = transport.clone();
        let version = version_a.clone();
        async move {
            dispatch(
                &transport,
                account,
                worker_id,
                &version,
                generation_a,
                "/hold?name=alpha&ms=3000",
            )
            .await
        }
    });
    let capability_in_flight = tokio::spawn({
        let transport = transport.clone();
        let version = version_a.clone();
        async move {
            dispatch(
                &transport,
                account,
                worker_id,
                &version,
                generation_a,
                "/rpc-pipeline-hold?name=alpha&ms=3000",
            )
            .await
        }
    });
    let admitted_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let admitted = dispatch(
            transport,
            account,
            worker_id,
            version_a,
            generation_a,
            "/hold-started?name=alpha",
        )
        .await;
        assert_eq!(admitted.status, 200, "{}", admitted.body);
        let admitted: serde_json::Value = serde_json::from_str(&admitted.body).unwrap();
        if admitted["fetch"] == true && admitted["capability"] == true {
            break;
        }
        assert!(
            Instant::now() < admitted_deadline,
            "old-generation operations were not admitted before promotion: {admitted}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let version_b = deploy(
        versions,
        version_request(
            account,
            worker_id,
            counter,
            other,
            output_queue_resource,
            "deploy-b",
            "B",
            30,
            true,
        ),
        supervisor,
    )
    .await;
    let completed_in_flight = in_flight.await.unwrap();
    assert_eq!(
        (
            completed_in_flight.status,
            completed_in_flight.body.as_str()
        ),
        (200, "A:2")
    );
    let completed_capability = capability_in_flight.await.unwrap();
    assert_eq!(
        (
            completed_capability.status,
            completed_capability.body.as_str()
        ),
        (200, "A:ok"),
        "an admitted old-generation RPC capability must remain pinned until its request completes"
    );
    let generation_b = workers
        .get_worker(account, worker_id)
        .unwrap()
        .route_generation;
    assert!(generation_b > generation_a);
    let promoted = dispatch(
        transport,
        account,
        worker_id,
        &version_b,
        generation_b,
        "/increment?name=alpha",
    )
    .await;
    assert_eq!((promoted.status, promoted.body.as_str()), (200, "B:3"));
    let promoted_capability = dispatch(
        transport,
        account,
        worker_id,
        &version_b,
        generation_b,
        "/rpc-capability?name=alpha",
    )
    .await;
    assert_rpc_capability(&promoted_capability, "B");
    let stale = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/increment?name=alpha",
    )
    .await;
    assert_eq!(stale.status, 500);
    (version_b, generation_b)
}

pub(super) async fn rollback_and_restart(
    case: &MatrixCase<'_>,
    version_b: &VersionRecord,
    generation_b: u64,
) -> u64 {
    let transport = case.transport;
    let supervisor = case.supervisor;
    let storage = case.storage;
    let account = case.account;
    let worker_id = case.worker.id;
    let _counter = case.counter;
    let _other = case.other;
    let _output_queue_resource = case.output_queue_resource;
    let version_a = case.version_a;
    let _generation_a = case.generation_a;
    let workers = WorkerRepository::new(storage.db());
    workers
        .promote_checked(
            account,
            worker_id,
            version_a.id,
            Some(version_b.id),
            Some(generation_b),
            RequestId::generate(),
            40,
        )
        .unwrap();
    let generation_rollback = workers
        .get_worker(account, worker_id)
        .unwrap()
        .route_generation;
    let rolled = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_rollback,
        "/rpc?name=alpha",
    )
    .await;
    assert_eq!((rolled.status, rolled.body.as_str()), (200, "A:3"));

    let old_pid = supervisor.snapshot().pid.unwrap();
    supervisor.report_unhealthy();
    wait_pid_change(supervisor, old_pid, Duration::from_secs(30)).await;
    let recovered = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_rollback,
        "/rpc?name=alpha",
    )
    .await;
    assert_eq!((recovered.status, recovered.body.as_str()), (200, "A:3"));
    let capability_after_restart = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_rollback,
        "/rpc-capability?name=alpha",
    )
    .await;
    assert_rpc_capability(&capability_after_restart, "A");

    let pending_capability = tokio::spawn({
        let transport = transport.clone();
        let version = version_a.clone();
        async move {
            dispatch(
                &transport,
                account,
                worker_id,
                &version,
                generation_rollback,
                "/rpc-pipeline-hold?name=alpha&ms=60000",
            )
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let old_pid = supervisor.snapshot().pid.unwrap();
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(old_pid).unwrap(),
        rustix::process::Signal::KILL,
    )
    .unwrap();
    wait_pid_change(supervisor, old_pid, Duration::from_secs(30)).await;
    match tokio::time::timeout(Duration::from_secs(10), pending_capability).await {
        Ok(Ok(response)) => assert_ne!(
            (response.status, response.body.as_str()),
            (200, "A:ok"),
            "an RPC capability from a dead runtime generation remained callable"
        ),
        Ok(Err(_)) => {}
        Err(_) => panic!("dead runtime-generation RPC capability did not settle"),
    }
    let fresh_capability = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_rollback,
        "/rpc-capability?name=alpha",
    )
    .await;
    assert_rpc_capability(&fresh_capability, "A");
    generation_rollback
}

pub(super) async fn verify_recovery(case: &MatrixCase<'_>, generation_rollback: u64) {
    let transport = case.transport;
    let supervisor = case.supervisor;
    let _storage = case.storage;
    let account = case.account;
    let worker_id = case.worker.id;
    let _counter = case.counter;
    let _other = case.other;
    let _output_queue_resource = case.output_queue_resource;
    let version_a = case.version_a;
    let _generation_a = case.generation_a;
    let scheduler = case.scheduler;
    let output_queue = case.output_queue;
    recovery::check(
        transport,
        supervisor,
        account,
        worker_id,
        version_a,
        generation_rollback,
    )
    .await;
    hibernation::check(
        transport,
        supervisor,
        account,
        worker_id,
        version_a,
        generation_rollback,
    )
    .await;
    output_crash::check(
        transport,
        supervisor,
        scheduler,
        output_crash::Target {
            queue: output_queue,
            account,
            worker: worker_id,
            version: version_a,
            generation: generation_rollback,
        },
    )
    .await;
}

pub(super) async fn delete_objects(
    case: &MatrixCase<'_>,
    named_id: &str,
    generation_rollback: u64,
) {
    let transport = case.transport;
    let _supervisor = case.supervisor;
    let storage = case.storage;
    let account = case.account;
    let worker_id = case.worker.id;
    let counter = case.counter;
    let _other = case.other;
    let _output_queue_resource = case.output_queue_resource;
    let version_a = case.version_a;
    let _generation_a = case.generation_a;
    let workers = WorkerRepository::new(storage.db());
    let object_id = DurableObjectId::from_str(named_id).unwrap();
    let repository = DurableObjectRepository::new(storage);
    let fenced = repository
        .begin_object_delete(account, counter, object_id, 50)
        .unwrap();
    let authority = repository
        .deletion_authority(account, counter, object_id, fenced.generation)
        .unwrap();
    transport.delete_durable_object(&authority).await.unwrap();
    repository
        .finish_object_delete(counter, object_id, fenced.generation, 51)
        .unwrap();
    let recreated = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_rollback,
        "/increment?name=alpha",
    )
    .await;
    assert_eq!((recreated.status, recreated.body.as_str()), (200, "A:1"));
    let alpha_generations = repository
        .list_objects(account, counter)
        .unwrap()
        .into_iter()
        .filter(|object| object.object_id == object_id)
        .map(|object| object.generation)
        .collect::<Vec<_>>();
    assert_eq!(alpha_generations, vec![1, 2]);

    let expected = workers
        .list_versions(account, worker_id)
        .unwrap()
        .into_iter()
        .filter(|version| version.deleted_at_ms.is_none())
        .map(|version| version.id)
        .collect::<Vec<_>>();
    workers
        .delete_worker(account, worker_id, &expected, RequestId::generate(), 60)
        .unwrap();
    let fenced_after_worker_delete = repository
        .begin_object_delete(account, counter, object_id, 61)
        .unwrap();
    let purge_authority = repository
        .deletion_authority(
            account,
            counter,
            object_id,
            fenced_after_worker_delete.generation,
        )
        .unwrap();
    transport
        .delete_durable_object(&purge_authority)
        .await
        .unwrap();
    repository
        .finish_object_delete(
            counter,
            object_id,
            fenced_after_worker_delete.generation,
            62,
        )
        .unwrap();
}
