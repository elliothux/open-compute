use super::*;
use crate::scheduler::{AlarmProjection, ClaimResult, SchedulerStore};
use rusqlite::Connection;
use uuid::Uuid;

fn create_object(
    repo: DurableObjectRepository<'_>,
    fixture: &Fixture,
    fill: u8,
) -> DurableObjectId {
    let object = public_id(fixture.namespace, fill);
    repo.authorize_dispatch(
        fixture.binding,
        fixture.version,
        &fixture.descriptor,
        object,
        21,
        true,
    )
    .unwrap();
    object
}

#[test]
fn alarm_repair_fences_stale_finishes_and_detects_projection_drift() {
    let (_temp, storage) = storage();
    let fixture = ready_fixture(&storage);
    let repo = DurableObjectRepository::new(&storage);
    let namespace = fixture.namespace;
    let object = create_object(repo, &fixture, 1);
    let path = storage.data_dir().control_db_path();
    let store = SchedulerStore::open(&path, 100, 21, fixture.account).unwrap();
    let alarm = AlarmProjection {
        namespace_resource_id: namespace,
        object_id: object,
        object_generation: 1,
        row_token: Uuid::now_v7().to_string(),
        due_at_ms: 40,
        target_version_id: fixture.version,
        execution_generation: fixture.route_generation,
        retry_count: 0,
    };
    assert_eq!(
        store
            .begin_alarm_repair(namespace, object, 1, "bad-revision")
            .unwrap_err()
            .code(),
        ErrorCode::SchedulerInternalProtocolError,
    );
    assert_eq!(
        store
            .finish_alarm_repair(namespace, object, 1, "bad-revision", None, 21)
            .unwrap_err()
            .code(),
        ErrorCode::SchedulerInternalProtocolError,
    );
    let stale = Uuid::now_v7().to_string();
    let revision = Uuid::now_v7().to_string();
    store
        .begin_alarm_repair(namespace, object, 1, &stale)
        .unwrap();
    store
        .begin_alarm_repair(namespace, object, 1, &revision)
        .unwrap();
    store
        .finish_alarm_repair(namespace, object, 1, &stale, Some(&alarm), 22)
        .unwrap();
    assert!(store.claim_due(40, 1).unwrap().is_empty());
    assert_eq!(repo.pending_alarm_repairs(None, 1).unwrap()[0].1, revision);

    store
        .finish_alarm_repair(namespace, object, 1, &revision, Some(&alarm), 22)
        .unwrap();
    store
        .finish_alarm_repair(namespace, object, 1, &stale, None, 23)
        .unwrap();
    assert!(repo.pending_alarm_repairs(None, 1).unwrap().is_empty());

    let job = store.claim_due(40, 1).unwrap().pop().unwrap();
    assert!(
        store
            .finish_claim(
                &job,
                ClaimResult::Reschedule {
                    due_at_ms: 50,
                    retry_count: 1,
                    last_error_code: Some("DO_RUNTIME_EXCEPTION"),
                },
                40,
            )
            .unwrap()
    );
    assert!(repo.pending_alarm_repairs(None, 1).unwrap().is_empty());

    let connection = Connection::open(&path).unwrap();
    let job = store.claim_due(50, 1).unwrap().pop().unwrap();
    connection
        .execute("UPDATE scheduled_jobs SET due_at_ms = 51", [])
        .unwrap();
    assert!(repo.pending_alarm_repairs(None, 1).unwrap().is_empty());
    assert!(
        store
            .finish_claim(
                &job,
                ClaimResult::MarkDiscarding {
                    last_error_code: "DO_RUNTIME_EXCEPTION"
                },
                51,
            )
            .unwrap()
    );
    assert!(repo.pending_alarm_repairs(None, 1).unwrap().is_empty());
    assert!(store.finish_discarding(&job).unwrap());
    assert_eq!(repo.pending_alarm_repairs(None, 1).unwrap()[0].1, revision);
    store
        .finish_alarm_repair(namespace, object, 1, &revision, Some(&alarm), 52)
        .unwrap();
    for sql in [
        "UPDATE scheduled_jobs SET row_token = 'different-row-token'",
        "UPDATE scheduled_jobs SET due_at_ms = 51",
        "UPDATE scheduled_jobs SET retry_count = 2",
        "DELETE FROM scheduled_jobs",
    ] {
        connection.execute(sql, []).unwrap();
        assert_eq!(repo.pending_alarm_repairs(None, 1).unwrap()[0].1, revision);
        store
            .finish_alarm_repair(namespace, object, 1, &revision, Some(&alarm), 41)
            .unwrap();
        assert!(repo.pending_alarm_repairs(None, 1).unwrap().is_empty());
    }
    store
        .finish_alarm_repair(namespace, object, 1, &revision, None, 42)
        .unwrap();
    store
        .finish_alarm_repair(namespace, object, 1, &revision, Some(&alarm), 43)
        .unwrap();
    assert!(store.claim_due(100, 1).unwrap().is_empty());
    assert!(repo.pending_alarm_repairs(None, 1).unwrap().is_empty());
}

#[test]
fn alarm_repair_backfills_live_generations_once_and_pages_after_reopen() {
    let (_temp, storage) = storage();
    let fixture = ready_fixture(&storage);
    let repo = DurableObjectRepository::new(&storage);
    let namespace = fixture.namespace;
    let creating = create_object(repo, &fixture, 1);
    let ready = create_object(repo, &fixture, 2);
    let deleting = create_object(repo, &fixture, 3);
    repo.finish_object_create(namespace, ready, 1, 22).unwrap();
    repo.begin_object_delete(fixture.account, namespace, deleting, 22)
        .unwrap();

    let path = storage.data_dir().control_db_path();
    let connection = Connection::open(&path).unwrap();
    // Restore the V26 schema while retaining the lifecycle fixture rows.
    connection
        .execute_batch(
            "DROP TABLE do_alarm_repair;
         DELETE FROM refinery_schema_history WHERE version = 27;",
        )
        .unwrap();
    drop(connection);
    let store = SchedulerStore::open(&path, 100, 23, fixture.account).unwrap();
    let first = repo.pending_alarm_repairs(None, 1).unwrap();
    assert_eq!(first[0].0.object_id, creating);
    assert!(Uuid::parse_str(&first[0].1).is_ok());
    let after = Some((namespace, creating, 1));
    let second = repo.pending_alarm_repairs(after, 10).unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].0.object_id, ready);
    assert_ne!(first[0].1, second[0].1);

    store
        .finish_alarm_repair(namespace, creating, 1, &first[0].1, None, 24)
        .unwrap();
    drop(store);
    let store = SchedulerStore::open(&path, 100, 25, fixture.account).unwrap();
    assert_eq!(repo.pending_alarm_repairs(None, 10).unwrap(), second);

    repo.begin_object_delete(fixture.account, namespace, ready, 26)
        .unwrap();
    assert_eq!(
        store
            .begin_alarm_repair(namespace, ready, 1, &Uuid::now_v7().to_string())
            .unwrap_err()
            .code(),
        ErrorCode::DoObjectDeleting,
    );
    assert_eq!(
        repo.finish_object_delete(namespace, ready, 1, 27)
            .unwrap()
            .state,
        DurableObjectState::Tombstoned,
    );
    assert_eq!(
        store
            .begin_alarm_repair(namespace, ready, 1, &Uuid::now_v7().to_string())
            .unwrap_err()
            .code(),
        ErrorCode::DoObjectDeleting,
    );
    store
        .finish_alarm_repair(namespace, ready, 1, &second[0].1, None, 27)
        .unwrap();
    let connection = Connection::open(&path).unwrap();
    let count: i64 = connection
        .query_row("SELECT count(*) FROM do_alarm_repair", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}
