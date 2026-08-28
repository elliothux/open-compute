//! SQLite aborts inside real control transactions cannot publish half a saga boundary.

use super::*;

#[test]
fn workflow_version_switch_failure_preserves_current_and_frozen_instances() {
    let (_temp, storage, deployment) = setup();
    let definition = ready(&storage, deployment);
    let repository = WorkflowRepository::new(storage.db());
    let account = storage.identity().default_account_id;
    let limits = WorkflowsConfig::default();
    let original = repository
        .reserve_instance(account, definition.id, Some("old"), 1, &limits, 10)
        .unwrap();
    repository
        .finalize_instance(&original.identity, 11)
        .unwrap();
    let replacement = staging(&storage, original.identity.target.worker_id);
    let workers = WorkerRepository::new(storage.db());
    workers.begin_validation(replacement).unwrap();
    workers.mark_ready(replacement, 12).unwrap();
    let version = repository
        .stage_version(account, definition.id, replacement, "Replacement", 1, 13)
        .unwrap();
    storage.db().with_read(|connection| {
        connection.execute_batch("CREATE TEMP TRIGGER reject_workflow_promotion AFTER UPDATE ON workflow_definitions
            WHEN OLD.current_version_id IS NOT NEW.current_version_id BEGIN SELECT RAISE(ABORT,'test transaction fault'); END;").unwrap();
        Ok(())
    }).unwrap();
    assert!(
        repository
            .finish_version(account, version.target.version_id, true, 14)
            .is_err()
    );
    assert_eq!(
        repository
            .definition(account, definition.id)
            .unwrap()
            .current_version_id,
        definition.current_version_id
    );
    assert_eq!(
        repository
            .version(account, version.target.version_id)
            .unwrap()
            .state,
        DeploymentState::Validating
    );
    assert_eq!(
        repository
            .reservation(original.identity.instance_id)
            .unwrap()
            .unwrap()
            .identity,
        original.identity
    );
    storage
        .db()
        .with_read(|connection| {
            connection
                .execute_batch("DROP TRIGGER reject_workflow_promotion;")
                .unwrap();
            Ok(())
        })
        .unwrap();
    repository
        .finish_version(account, version.target.version_id, true, 15)
        .unwrap();
    let next = repository
        .reserve_instance(account, definition.id, Some("new"), 1, &limits, 16)
        .unwrap();
    assert_eq!(next.identity.target.deployment_id, replacement);
    assert_eq!(next.identity.target.version_id, version.target.version_id);
    assert_eq!(
        repository
            .reservation(original.identity.instance_id)
            .unwrap()
            .unwrap()
            .identity
            .target
            .deployment_id,
        deployment
    );
    assert_eq!(repository.retire_unused_versions(32, 17).unwrap(), 0);
    assert!(
        repository
            .instance_referrers_intact(&original.identity)
            .unwrap()
    );
}

#[test]
fn workflow_control_commit_failure_rolls_back_reservation_finalize_and_release() {
    let (_temp, storage, deployment) = setup();
    let definition = ready(&storage, deployment);
    let repository = WorkflowRepository::new(storage.db());
    let account = storage.identity().default_account_id;
    let limits = WorkflowsConfig::default();
    let inject = |sql: &str| {
        storage
            .db()
            .with_read(|connection| {
                connection.execute_batch(sql).unwrap();
                Ok(())
            })
            .unwrap();
    };
    inject(
        "CREATE TEMP TRIGGER reject_workflow_reservation AFTER INSERT ON workflow_referrers
        WHEN NEW.referrer_kind='instance' BEGIN SELECT RAISE(ABORT,'test transaction fault'); END;",
    );
    assert!(
        repository
            .reserve_instance(account, definition.id, Some("atomic"), 1, &limits, 10)
            .is_err()
    );
    assert_eq!(
        repository
            .find_instance(definition.id, "atomic")
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInstanceNotFound
    );
    storage
        .db()
        .with_read(|connection| {
            for sql in [
                "SELECT COUNT(*) FROM workflow_instance_referrers",
                "SELECT COUNT(*) FROM workflow_referrers WHERE referrer_kind='instance'",
                "SELECT COUNT(*) FROM deployment_referrers WHERE kind='workflow_instance'",
            ] {
                assert_eq!(
                    connection
                        .query_row(sql, [], |row| row.get::<_, u64>(0))
                        .unwrap(),
                    0
                );
            }
            Ok(())
        })
        .unwrap();
    inject("DROP TRIGGER reject_workflow_reservation;");
    let reservation = repository
        .reserve_instance(account, definition.id, Some("atomic"), 1, &limits, 10)
        .unwrap();
    inject(
        "CREATE TEMP TRIGGER reject_workflow_finalize AFTER UPDATE ON workflow_instance_referrers
        WHEN NEW.state='live' BEGIN SELECT RAISE(ABORT,'test transaction fault'); END;",
    );
    assert!(
        repository
            .finalize_instance(&reservation.identity, 11)
            .is_err()
    );
    assert_eq!(
        repository
            .reservation(reservation.identity.instance_id)
            .unwrap()
            .unwrap()
            .state,
        WorkflowRefState::Creating
    );
    assert!(
        repository
            .instance_referrers_intact(&reservation.identity)
            .unwrap()
    );
    inject("DROP TRIGGER reject_workflow_finalize;");
    repository
        .finalize_instance(&reservation.identity, 11)
        .unwrap();
    inject(
        "CREATE TEMP TRIGGER reject_workflow_release AFTER UPDATE ON workflow_instance_referrers
        WHEN NEW.state='released' BEGIN SELECT RAISE(ABORT,'test transaction fault'); END;",
    );
    assert!(
        repository
            .release_instance(&reservation.identity, 12)
            .is_err()
    );
    assert_eq!(
        repository
            .reservation(reservation.identity.instance_id)
            .unwrap()
            .unwrap()
            .state,
        WorkflowRefState::Live
    );
    assert!(
        repository
            .instance_referrers_intact(&reservation.identity)
            .unwrap()
    );
    inject("DROP TRIGGER reject_workflow_release;");
    repository
        .release_instance(&reservation.identity, 12)
        .unwrap();
    repository
        .release_instance(&reservation.identity, 13)
        .unwrap();
    assert_eq!(
        repository
            .reservation(reservation.identity.instance_id)
            .unwrap()
            .unwrap()
            .state,
        WorkflowRefState::Released
    );
    assert!(
        !repository
            .instance_referrers_intact(&reservation.identity)
            .unwrap()
    );
}
