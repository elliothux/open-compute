use super::*;
use open_compute_core::SystemClock;

fn database(path: &std::path::Path) -> (ControlDb, InstanceId) {
    let db = ControlDb::open(path, 1000).unwrap();
    db.migrate(&SystemClock).unwrap();
    let identity = crate::identity::bootstrap(&db, &SystemClock, &"a".repeat(64)).unwrap();
    (db, identity.instance_id)
}

fn record(instance: InstanceId) -> BrowserSessionRecord {
    BrowserSessionRecord {
        id: uuid::Uuid::now_v7().to_string(),
        instance_id: instance,
        generation: uuid::Uuid::now_v7().to_string(),
        contract_sha256: [7; 32],
        state: BrowserSessionState::Ready,
        keep_alive_ms: 60_000,
        connections: 0,
        created_at_ms: 1,
        last_activity_at_ms: 1,
        connected_at_ms: None,
        closed_at_ms: None,
        close_reason: None,
    }
}

#[test]
fn browser_history_retention_bounds_terminal_rows_without_touching_live_authority() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("control.sqlite");
    let (db, instance) = database(&path);
    let sessions = BrowserSessions::new(&db);
    let live = [record(instance), record(instance), record(instance)];
    for row in &live {
        sessions.create(row, 10).unwrap();
    }
    sessions
        .connection(instance, &live[1].id, &live[1].generation, true, 10)
        .unwrap();
    sessions
        .begin_close(instance, &live[2].id, &live[2].generation)
        .unwrap();
    let mut terminal = Vec::new();
    for (time, reason) in [
        (50, BrowserSessionCloseReason::Normal),
        (100, BrowserSessionCloseReason::Idle),
        (200, BrowserSessionCloseReason::Lost),
        (300, BrowserSessionCloseReason::Normal),
        (300, BrowserSessionCloseReason::Idle),
    ] {
        let row = record(instance);
        sessions.create(&row, 10).unwrap();
        sessions
            .begin_close(instance, &row.id, &row.generation)
            .unwrap();
        sessions
            .finish_close(instance, &row.id, &row.generation, reason, time)
            .unwrap();
        terminal.push(row.id);
    }
    for invalid in [0, 100_001] {
        assert!(sessions.retain_history(instance, 0, invalid).is_err());
    }
    assert!(
        sessions
            .retain_history(InstanceId::generate(), i64::MAX, 1)
            .is_err()
    );
    assert_eq!(sessions.list(instance, true, 10, 0).unwrap().len(), 5);
    assert_eq!(sessions.list(instance, false, 10, 0).unwrap().len(), 3);
    assert_eq!(sessions.retain_history(instance, 100, 10).unwrap(), 2);
    assert_eq!(sessions.retain_history(instance, 100, 2).unwrap(), 1);
    assert_eq!(sessions.retain_history(instance, 100, 2).unwrap(), 0);
    let mut expected = terminal[3..].to_vec();
    expected.sort();
    let remaining = sessions.list(instance, true, 10, 0).unwrap();
    assert_eq!(remaining.len(), 2);
    assert_eq!(
        remaining
            .iter()
            .map(|row| row.id.clone())
            .collect::<Vec<_>>(),
        expected
    );
    for (row, state) in live.iter().zip([
        BrowserSessionState::Ready,
        BrowserSessionState::Connected,
        BrowserSessionState::Closing,
    ]) {
        assert_eq!(
            sessions
                .get(instance, &row.id, &row.generation)
                .unwrap()
                .unwrap()
                .state,
            state
        );
    }
    assert_eq!(sessions.retain_history(instance, 100, 1).unwrap(), 1);
    assert_eq!(
        sessions
            .list(instance, true, 10, 0)
            .unwrap()
            .last()
            .unwrap()
            .id,
        *expected.last().unwrap()
    );
    assert_eq!(sessions.retain_history(instance, 300, 1).unwrap(), 1);
    drop(db);
    let db = ControlDb::open(&path, 1000).unwrap();
    let sessions = BrowserSessions::new(&db);
    assert!(sessions.list(instance, true, 10, 0).unwrap().is_empty());
    assert_eq!(sessions.list(instance, false, 10, 0).unwrap().len(), 3);
    assert_eq!(sessions.retain_history(instance, i64::MAX, 1).unwrap(), 0);
}

#[test]
fn browser_sessions_are_instance_and_generation_fenced_and_terminal() {
    let directory = tempfile::tempdir().unwrap();
    let (db, instance) = database(&directory.path().join("control.sqlite"));
    let sessions = BrowserSessions::new(&db);
    let record = record(instance);
    sessions.create(&record, 1).unwrap();
    assert_eq!(
        sessions.create(&record, 1).unwrap_err().code(),
        ErrorCode::AdmissionBusy
    );
    let other = InstanceId::generate();
    assert!(
        sessions
            .get(other, &record.id, &record.generation)
            .unwrap()
            .is_none()
    );
    assert!(
        sessions
            .get(instance, &record.id, &uuid::Uuid::now_v7().to_string())
            .unwrap()
            .is_none()
    );
    assert!(sessions.list(other, false, 10, 0).unwrap().is_empty());
    assert!(
        sessions
            .connection(other, &record.id, &record.generation, true, 2)
            .is_err()
    );
    sessions
        .connection(instance, &record.id, &record.generation, true, 2)
        .unwrap();
    sessions
        .connection(instance, &record.id, &record.generation, true, 3)
        .unwrap();
    let current = sessions
        .get(instance, &record.id, &record.generation)
        .unwrap()
        .unwrap();
    assert_eq!(current.state, BrowserSessionState::Connected);
    assert_eq!(current.connections, 2);
    assert_eq!(current.last_activity_at_ms, 1);
    sessions
        .activity(instance, &record.id, &record.generation, 4)
        .unwrap();
    sessions
        .connection(instance, &record.id, &record.generation, false, 5)
        .unwrap();
    sessions
        .connection(instance, &record.id, &record.generation, false, 5)
        .unwrap();
    assert!(
        sessions
            .connection(instance, &record.id, &record.generation, false, 5)
            .is_err()
    );
    let current = sessions
        .get(instance, &record.id, &record.generation)
        .unwrap()
        .unwrap();
    assert_eq!(current.state, BrowserSessionState::Ready);
    assert_eq!(current.last_activity_at_ms, 4);
    assert!(
        sessions
            .begin_close(instance, &record.id, &record.generation)
            .unwrap()
    );
    assert!(
        !sessions
            .begin_close(instance, &record.id, &record.generation)
            .unwrap()
    );
    assert!(
        sessions
            .activity(instance, &record.id, &record.generation, 6)
            .is_err()
    );
    assert!(
        sessions
            .connection(instance, &record.id, &record.generation, true, 6)
            .is_err()
    );
    sessions
        .finish_close(
            instance,
            &record.id,
            &record.generation,
            BrowserSessionCloseReason::Idle,
            6,
        )
        .unwrap();
    sessions
        .finish_close(
            instance,
            &record.id,
            &record.generation,
            BrowserSessionCloseReason::Normal,
            7,
        )
        .unwrap();
    assert!(sessions.list(instance, false, 10, 0).unwrap().is_empty());
    let history = sessions.list(instance, true, 10, 0).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].closed_at_ms, Some(6));
    assert_eq!(history[0].state, BrowserSessionState::Closed);
    assert_eq!(
        history[0].close_reason,
        Some(BrowserSessionCloseReason::Idle)
    );
    assert!(
        db.with_immediate(|tx| {
            tx.execute(
                "UPDATE browser_sessions SET state='ready', closed_at_ms=NULL WHERE id=?1",
                [&record.id],
            )
            .map_err(|_| invariant())?;
            Ok(())
        })
        .is_err()
    );
}

#[test]
fn browser_session_restart_discards_old_generation_without_locators_or_resurrection() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("control.sqlite");
    let (db, instance) = database(&path);
    let first = record(instance);
    let sessions = BrowserSessions::new(&db);
    sessions.create(&first, 8).unwrap();
    sessions
        .connection(instance, &first.id, &first.generation, true, 2)
        .unwrap();
    let second = record(instance);
    sessions.create(&second, 8).unwrap();
    assert!(
        sessions
            .begin_close(instance, &second.id, &second.generation)
            .unwrap()
    );
    drop(db);
    let (db, same_instance) = database(&path);
    assert_eq!(instance, same_instance);
    let sessions = BrowserSessions::new(&db);
    assert_eq!(sessions.lose_all(instance, 3).unwrap(), 2);
    assert_eq!(sessions.lose_all(instance, 4).unwrap(), 0);
    let history = sessions.list(instance, true, 10, 0).unwrap();
    assert_eq!(history.len(), 2);
    assert!(
        history
            .iter()
            .all(|row| row.state == BrowserSessionState::Lost
                && row.connections == 0
                && row.closed_at_ms == Some(3))
    );
    assert!(
        sessions
            .connection(instance, &first.id, &first.generation, true, 4)
            .is_err()
    );
    sessions.create(&record(instance), 1).unwrap();
    assert!(sessions.list(instance, true, 0, 0).is_err());
    assert!(sessions.list(instance, true, 1025, 0).is_err());
    assert!(sessions.list(instance, true, 10, 1_000_001).is_err());
    for mutate in 0..5 {
        let mut invalid = record(instance);
        match mutate {
            0 => invalid.id = "bad".into(),
            1 => invalid.generation = "bad".into(),
            2 => invalid.keep_alive_ms = 0,
            3 => invalid.state = BrowserSessionState::Lost,
            _ => invalid.connections = 1,
        }
        assert!(sessions.create(&invalid, 8).is_err());
    }
    let mut invalid = record(InstanceId::generate());
    assert!(sessions.create(&invalid, 8).is_err());
    invalid.instance_id = instance;
    assert!(sessions.create(&invalid, 0).is_err());
    assert!(BrowserSessionState::parse("invalid").is_err());
}
