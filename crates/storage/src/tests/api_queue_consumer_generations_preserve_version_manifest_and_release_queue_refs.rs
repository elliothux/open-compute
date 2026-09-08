use super::*;

#[test]
fn api_queue_consumer_generations_preserve_version_manifest_and_release_queue_refs() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let request = open_compute_core::RequestId::generate();
    let queue_id = open_compute_core::QueueId::generate();
    let queues = crate::QueueRepository::new(storage.db());
    queues
        .insert_creating(
            account,
            queue_id,
            "api-consumer",
            crate::QueueConfig::default(),
            1,
        )
        .unwrap();
    queues.mark_ready(account, queue_id, 2).unwrap();
    assert!(
        queues
            .set_delivery_paused(account, queue_id, true, 3)
            .unwrap()
            .delivery_paused
    );

    let workers = WorkerRepository::new(storage.db());
    let (first, _) = workers
        .create_worker(account, "api-consumer-a", request, 4, 1_000_000)
        .unwrap();
    let first_version = insert_ready(&workers, account, first.id, [1; 32], request, 5);
    workers
        .promote(account, first.id, first_version, None, request, 7)
        .unwrap();
    let (second, _) = workers
        .create_worker(account, "api-consumer-b", request, 8, 1_000_000)
        .unwrap();
    let second_version = insert_ready(&workers, account, second.id, [2; 32], request, 9);
    workers
        .promote(account, second.id, second_version, None, request, 11)
        .unwrap();

    let repository = QueueConsumerRepository::new(storage.db());
    let make_declaration = |id| NewQueueConsumerDeclaration {
        id,
        queue_id,
        queue_lifecycle_generation: 1,
        entrypoint: None,
        config: QueueConsumerConfig::default(),
        dead_letter_queue: None,
        capability_version: 1,
        descriptor_sha256: [3; 32],
    };
    let first_declaration = repository
        .create_api_declaration(
            first_version,
            &make_declaration(QueueConsumerId::generate()),
            12,
        )
        .unwrap();
    assert!(
        repository
            .version_declarations(first_version)
            .unwrap()
            .is_empty()
    );
    let consumer = repository
        .create_attachment(account, first.id, &first_declaration, 13)
        .unwrap();
    assert!(repository.finish_activation(consumer.id, 1, 14).unwrap());

    let second_declaration = repository
        .create_api_declaration(
            second_version,
            &make_declaration(QueueConsumerId::generate()),
            15,
        )
        .unwrap();
    assert!(
        repository
            .begin_update(consumer.id, 1, second.id, &second_declaration, 16)
            .unwrap()
    );
    let pending = repository.get(consumer.id).unwrap();
    assert_eq!(pending.pending_worker_id, Some(second.id));
    assert!(
        repository
            .switch_target(consumer.id, 2, &second_declaration, 17)
            .unwrap()
    );
    assert!(repository.finish_update(consumer.id, 2, false, 18).unwrap());
    assert_eq!(repository.get(consumer.id).unwrap().worker_id, second.id);
    assert!(repository.begin_delete(consumer.id, 2, 19).unwrap());
    assert!(repository.finish_delete(consumer.id, 2, 20).unwrap());
    queues.begin_delete(account, queue_id, 1, 21).unwrap();
}
