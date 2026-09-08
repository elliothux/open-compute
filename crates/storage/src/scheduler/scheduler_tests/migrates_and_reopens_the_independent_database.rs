use super::*;

#[test]
fn migrates_and_reopens_the_independent_database() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("scheduler.sqlite");
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    let store = SchedulerStore::open(&path, 100, 10).unwrap();
    assert_eq!(store.summary(10).unwrap(), SchedulerSummary::default());
    drop(store);
    let reopened = SchedulerStore::open(&path, 100, 20).unwrap();
    reopened.quick_check().unwrap();
    drop(reopened);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE scheduler_migrations SET checksum_sha256 = zeroblob(32) WHERE version = 1",
            [],
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        SchedulerStore::open(&path, 100, 30).unwrap_err().code(),
        ErrorCode::SchedulerCorrupt
    );
}
