use super::*;

#[test]
fn p2_2_queue_catalog_and_create_idempotency_boundaries_are_complete() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let repository = crate::QueueRepository::new(storage.db());
    let workers = WorkerRepository::new(storage.db());
    let fingerprint = [7_u8; 32];
    let other_fingerprint = [8_u8; 32];

    assert_eq!(crate::QueueState::Creating.as_str(), "creating");
    assert_eq!(crate::QueueState::Ready.as_str(), "ready");
    assert_eq!(crate::QueueState::Deleting.as_str(), "deleting");
    assert_eq!(crate::QueueState::Tombstoned.as_str(), "tombstoned");
    assert_eq!(
        "creating".parse::<crate::QueueState>().unwrap(),
        crate::QueueState::Creating
    );
    assert_eq!(
        "tombstoned".parse::<crate::QueueState>().unwrap(),
        crate::QueueState::Tombstoned
    );
    assert_eq!(
        "invalid".parse::<crate::QueueState>().unwrap_err().code(),
        ErrorCode::QueueInvariantViolation
    );
    assert_eq!(crate::QueueAvailability::Healthy.as_str(), "healthy");
    assert_eq!(crate::QueueAvailability::Degraded.as_str(), "degraded");
    assert_eq!(
        crate::QueueAvailability::Unavailable.as_str(),
        "unavailable"
    );
    assert_eq!(
        "degraded".parse::<crate::QueueAvailability>().unwrap(),
        crate::QueueAvailability::Degraded
    );
    assert_eq!(
        "invalid"
            .parse::<crate::QueueAvailability>()
            .unwrap_err()
            .code(),
        ErrorCode::QueueInvariantViolation
    );

    for invalid in [
        crate::QueueConfig {
            delivery_delay_seconds: crate::QUEUE_MAX_DELAY_SECONDS + 1,
            ..crate::QueueConfig::default()
        },
        crate::QueueConfig {
            retention_seconds: crate::QUEUE_MIN_RETENTION_SECONDS - 1,
            ..crate::QueueConfig::default()
        },
        crate::QueueConfig {
            max_message_bytes: 0,
            ..crate::QueueConfig::default()
        },
        crate::QueueConfig {
            max_batch_messages: 0,
            ..crate::QueueConfig::default()
        },
        crate::QueueConfig {
            max_batch_bytes: 0,
            ..crate::QueueConfig::default()
        },
        crate::QueueConfig {
            max_backlog_bytes: 0,
            ..crate::QueueConfig::default()
        },
    ] {
        assert_eq!(
            invalid.validate().unwrap_err().code(),
            ErrorCode::LimitInvalid
        );
    }
    for name in ["", "bad\nname"] {
        assert_eq!(
            repository
                .insert_creating(
                    account,
                    open_compute_core::QueueId::generate(),
                    name,
                    crate::QueueConfig::default(),
                    1,
                )
                .unwrap_err()
                .code(),
            ErrorCode::ConfigInvalid
        );
    }
    assert_eq!(
        repository
            .list(
                account,
                None,
                None,
                CatalogSort::UpdatedAt,
                CatalogDirection::Desc,
                None,
                0,
            )
            .unwrap_err()
            .code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        repository.list_reconcile(1001).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        repository.list_running_mutations(0).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        repository
            .insert_creating(
                AccountId::generate(),
                open_compute_core::QueueId::generate(),
                "orphan",
                crate::QueueConfig::default(),
                1,
            )
            .unwrap_err()
            .code(),
        ErrorCode::AccountNotFound
    );

    let running_id = open_compute_core::QueueId::generate();
    let running = repository
        .reserve_create(
            account,
            running_id,
            "running",
            crate::QueueConfig::default(),
            "create-running",
            "key",
            &fingerprint,
            10,
            100,
            10,
        )
        .unwrap();
    assert!(matches!(
        running,
        crate::QueueCreateReservation::Reserved(_)
    ));
    assert_eq!(
        repository
            .reserve_create(
                account,
                running_id,
                "running",
                crate::QueueConfig::default(),
                "create-running",
                "key",
                &fingerprint,
                10,
                100,
                10,
            )
            .unwrap(),
        crate::QueueCreateReservation::Running
    );
    assert_eq!(
        repository
            .reserve_create(
                account,
                running_id,
                "running",
                crate::QueueConfig::default(),
                "create-running",
                "key",
                &other_fingerprint,
                10,
                100,
                10,
            )
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );

    let complete_id = open_compute_core::QueueId::generate();
    let complete = match repository
        .reserve_create(
            account,
            complete_id,
            "complete",
            crate::QueueConfig::default(),
            "create-complete",
            "key",
            &fingerprint,
            11,
            100,
            10,
        )
        .unwrap()
    {
        crate::QueueCreateReservation::Reserved(queue) => queue,
        other => panic!("unexpected reservation: {other:?}"),
    };
    repository
        .complete_reconciled_create(&complete, b"{\"complete\":true}")
        .unwrap();
    assert_eq!(
        repository
            .reserve_create(
                account,
                complete_id,
                "complete",
                crate::QueueConfig::default(),
                "create-complete",
                "key",
                &fingerprint,
                11,
                100,
                10,
            )
            .unwrap(),
        crate::QueueCreateReservation::Complete(b"{\"complete\":true}".to_vec())
    );

    let failed_id = open_compute_core::QueueId::generate();
    assert!(matches!(
        repository
            .reserve_create(
                account,
                failed_id,
                "failed",
                crate::QueueConfig::default(),
                "create-failed",
                "key",
                &fingerprint,
                12,
                100,
                10,
            )
            .unwrap(),
        crate::QueueCreateReservation::Reserved(_)
    ));
    workers
        .fail_idempotency(
            account,
            "queue.create",
            "create-failed",
            &fingerprint,
            b"{\"failed\":true}",
        )
        .unwrap();
    assert_eq!(
        repository
            .reserve_create(
                account,
                failed_id,
                "failed",
                crate::QueueConfig::default(),
                "create-failed",
                "key",
                &fingerprint,
                12,
                100,
                10,
            )
            .unwrap(),
        crate::QueueCreateReservation::Failed(b"{\"failed\":true}".to_vec())
    );
    assert_eq!(
        repository
            .reserve_create(
                account,
                open_compute_core::QueueId::generate(),
                "quota",
                crate::QueueConfig::default(),
                "create-quota",
                "key",
                &fingerprint,
                13,
                100,
                0,
            )
            .unwrap_err()
            .code(),
        ErrorCode::QuotaExceeded
    );
}

#[test]
fn p2_2_queue_lifecycle_and_mutation_boundaries_are_complete() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let repository = crate::QueueRepository::new(storage.db());
    let workers = WorkerRepository::new(storage.db());
    let fingerprint = [7_u8; 32];
    let other_fingerprint = [8_u8; 32];
    let lifecycle_id = open_compute_core::QueueId::generate();
    let lifecycle = repository
        .insert_creating(
            account,
            lifecycle_id,
            "lifecycle",
            crate::QueueConfig::default(),
            20,
        )
        .unwrap();
    assert_eq!(
        repository
            .get(AccountId::generate(), lifecycle_id)
            .unwrap_err()
            .code(),
        ErrorCode::QueueNotFound
    );
    assert_eq!(
        repository
            .rename(
                account,
                lifecycle_id,
                "too-early",
                open_compute_core::RequestId::generate(),
                21
            )
            .unwrap_err()
            .code(),
        ErrorCode::QueueNotReady
    );
    let ready = repository.mark_ready(account, lifecycle_id, 22).unwrap();
    assert_eq!(ready.state, crate::QueueState::Ready);
    assert_eq!(
        repository
            .mark_ready(account, lifecycle_id, 23)
            .unwrap_err()
            .code(),
        ErrorCode::QueueNotReady
    );
    assert_eq!(
        repository
            .write_config_pending(account, lifecycle_id, 9, ready.config, 24)
            .unwrap_err()
            .code(),
        ErrorCode::QueueConfigPending
    );
    assert_eq!(
        repository
            .mark_config_healthy(
                account,
                lifecycle_id,
                1,
                open_compute_core::RequestId::generate(),
                25,
            )
            .unwrap_err()
            .code(),
        ErrorCode::QueueConfigPending
    );
    assert_eq!(
        repository
            .begin_delete(account, lifecycle_id, 9, 26)
            .unwrap_err()
            .code(),
        ErrorCode::QueueNotReady
    );
    repository
        .begin_delete(account, lifecycle_id, 1, 27)
        .unwrap();
    assert_eq!(
        repository
            .begin_delete(account, lifecycle_id, 1, 28)
            .unwrap_err()
            .code(),
        ErrorCode::QueueNotReady
    );
    repository
        .mark_tombstoned(
            account,
            lifecycle_id,
            open_compute_core::RequestId::generate(),
            29,
        )
        .unwrap();
    assert_eq!(
        repository
            .mark_tombstoned(
                account,
                lifecycle_id,
                open_compute_core::RequestId::generate(),
                30,
            )
            .unwrap_err()
            .code(),
        ErrorCode::QueueNotReady
    );

    let mutation_id = lifecycle.id;
    let mutation = crate::RunningQueueMutation {
        account_id: account,
        scope: format!("queue.patch:{mutation_id}"),
        idempotency_key: "mutation".to_owned(),
        request_fingerprint: fingerprint,
        queue_id: mutation_id,
        intent_json: b"{\"version\":1}".to_vec(),
    };
    assert_eq!(
        repository
            .reserve_mutation(
                account,
                &mutation.scope,
                &mutation.idempotency_key,
                "key",
                &fingerprint,
                mutation_id,
                &mutation.intent_json,
                40,
                100,
            )
            .unwrap(),
        IdempotencyReservation::Reserved
    );
    assert_eq!(
        repository
            .reserve_mutation(
                account,
                &mutation.scope,
                &mutation.idempotency_key,
                "key",
                &fingerprint,
                mutation_id,
                &mutation.intent_json,
                40,
                100,
            )
            .unwrap(),
        IdempotencyReservation::Running
    );
    assert_eq!(
        repository.list_running_mutations(10).unwrap(),
        vec![mutation.clone()]
    );
    repository
        .replace_mutation_intent(&mutation, b"{\"version\":1,\"changed\":true}")
        .unwrap();
    let mut wrong = mutation.clone();
    wrong.request_fingerprint = other_fingerprint;
    assert_eq!(
        repository
            .replace_mutation_intent(&wrong, b"{}")
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );
    assert_eq!(
        repository
            .reserve_mutation(
                account,
                &mutation.scope,
                &mutation.idempotency_key,
                "key",
                &other_fingerprint,
                mutation_id,
                &mutation.intent_json,
                40,
                100,
            )
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );
    workers
        .complete_idempotency_with_queue_ref(
            account,
            &mutation.scope,
            &mutation.idempotency_key,
            &fingerprint,
            b"{\"done\":true}",
            mutation_id,
        )
        .unwrap();
    assert_eq!(
        repository
            .reserve_mutation(
                account,
                &mutation.scope,
                &mutation.idempotency_key,
                "key",
                &fingerprint,
                mutation_id,
                &mutation.intent_json,
                40,
                100,
            )
            .unwrap(),
        IdempotencyReservation::Complete(b"{\"done\":true}".to_vec())
    );
}
