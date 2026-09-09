use super::*;

pub(super) async fn exercise_retarget_and_repair(
    scenario: &Scenario<'_, '_>,
    runtime: &RuntimeFixture,
) {
    let Scenario {
        controller,
        target: request_target,
        storage,
        worker,
        ..
    } = scenario;
    let account = request_target.account;
    let queue_id = request_target.queue;
    let scheduler = &runtime.scheduler;
    let consumer_repo = open_compute_storage::QueueConsumerRepository::new(storage.db());
    let mut retarget = promotion_request::build(
        request_target,
        "p23-retarget",
        "retarget",
        true,
        "ignored",
        40,
    );
    retarget.crons = vec!["30 * * * *".to_owned()];
    let retargeted = controller.create_version(retarget).await.unwrap();
    let retargeted_id = match retargeted {
        CreateVersionOutcome::Applied(result) => result.version.id,
        CreateVersionOutcome::Replay(_) => panic!("retargeted P2.3 version replayed"),
    };
    let retargeted_consumer = consumer_repo.live_for_queue(queue_id).unwrap().unwrap();
    assert_eq!(retargeted_consumer.consumer_generation, 4);
    assert_eq!(retargeted_consumer.version_id, retargeted_id);
    let retargeted_crons = open_compute_storage::CronRepository::new(storage.db())
        .live_for_worker(worker.id)
        .unwrap();
    assert_eq!(retargeted_crons.len(), 1);
    assert_eq!(retargeted_crons[0].expression, "30 * * * *");
    assert_eq!(retargeted_crons[0].version_id, retargeted_id);
    let retargeted_declaration = consumer_repo
        .version_declarations(retargeted_id)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let retargeted_cron_declarations = vec![open_compute_storage::CronDeclaration {
        id: open_compute_core::CronActivationId::generate(),
        version_id: retargeted_id,
        expression: retargeted_crons[0].expression.clone(),
        expression_sha256: retargeted_crons[0].expression_sha256,
        parser_version: retargeted_crons[0].parser_version,
        scheduled_handler: true,
        workflow_bindings: Vec::new(),
        created_at_ms: 706_002,
    }];
    assert!(
        consumer_repo
            .begin_delete(
                retargeted_consumer.id,
                retargeted_consumer.consumer_generation,
                706_001,
            )
            .unwrap()
    );
    assert_eq!(
        open_compute_storage::CronRepository::new(storage.db())
            .retire_before(
                worker.id,
                retargeted_crons[0].activation_generation + 1,
                706_001,
            )
            .unwrap(),
        1
    );
    assert!(scheduler.repair_products(1_000).unwrap() >= 2);
    assert!(consumer_repo.live_for_queue(queue_id).unwrap().is_none());
    assert!(
        open_compute_storage::CronRepository::new(storage.db())
            .live_for_worker(worker.id)
            .unwrap()
            .is_empty()
    );
    let reactivated = consumer_repo
        .create_attachment(account, worker.id, &retargeted_declaration, 706_002)
        .unwrap();
    let restaged = open_compute_storage::CronRepository::new(storage.db())
        .stage_activations(
            account,
            worker.id,
            retargeted_id,
            retargeted_crons[0].activation_generation + 1,
            &retargeted_cron_declarations,
            706_002,
        )
        .unwrap();
    assert_eq!(restaged.len(), 1);
    assert!(scheduler.repair_products(1_000).unwrap() >= 2);
    assert_eq!(
        consumer_repo.get(reactivated.id).unwrap().state,
        open_compute_storage::QueueConsumerState::Active
    );
    assert_eq!(
        open_compute_storage::CronRepository::new(storage.db())
            .live_for_worker(worker.id)
            .unwrap()[0]
            .state,
        open_compute_storage::CronActivationState::Active
    );
    assert!(
        consumer_repo
            .begin_update(
                reactivated.id,
                reactivated.consumer_generation,
                worker.id,
                &retargeted_declaration,
                706_003,
            )
            .unwrap()
    );
    assert!(scheduler.repair_products(1_000).unwrap() >= 2);
    let reactivated = consumer_repo.get(reactivated.id).unwrap();
    assert_eq!(reactivated.consumer_generation, 2);
    assert_eq!(
        reactivated.state,
        open_compute_storage::QueueConsumerState::Active
    );
    assert!(
        consumer_repo
            .begin_delete(reactivated.id, reactivated.consumer_generation, 706_004)
            .unwrap()
    );
    assert_eq!(
        open_compute_storage::CronRepository::new(storage.db())
            .retire_before(worker.id, restaged[0].activation_generation + 1, 706_004,)
            .unwrap(),
        1
    );
    assert!(scheduler.repair_products(1_000).unwrap() >= 2);
}
