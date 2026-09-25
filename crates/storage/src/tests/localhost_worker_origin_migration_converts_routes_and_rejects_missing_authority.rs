use super::*;

fn seed_v5(path: &Path, include_route: bool) -> (InstanceId, WorkerId, String) {
    let mut connection = Connection::open(path).unwrap();
    crate::schema_migrations::migrate_to_for_test(
        &mut connection,
        crate::schema_migrations::DatabaseKind::Control,
        5,
    );
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    let account = InstanceId::generate();
    let worker = WorkerId::generate();
    let route = uuid::Uuid::now_v7().to_string();
    connection
        .execute(
            "INSERT INTO accounts(id, name, created_at_ms, deleted_at_ms)
             VALUES(?1, 'migration-account', 1, NULL)",
            [account.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO platform_meta(key, value, updated_at_ms)
         VALUES('instance_id', CAST(?1 AS BLOB), 1),
               ('created_at_ms', CAST('1' AS BLOB), 1)",
            [account.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO workers
             (id, account_id, name, active_deployment_id, do_storage_id,
              route_generation, created_at_ms, updated_at_ms, deleted_at_ms, ownership)
             VALUES(?1, ?2, 'app', NULL, ?3, 1, 2, 2, NULL, 'tenant')",
            [
                worker.to_string(),
                account.to_string(),
                uuid::Uuid::now_v7().to_string(),
            ],
        )
        .unwrap();
    if include_route {
        connection
            .execute(
                "INSERT INTO worker_routes
                 (id, account_id, worker_id, kind, hostname_ascii, path_prefix, entrypoint,
                  state, generation, created_at_ms, updated_at_ms, deleted_at_ms)
                 VALUES(?1, ?2, ?3, 'platform_path', NULL, ?4, NULL,
                        'active', 1, 3, 3, NULL)",
                rusqlite::params![
                    route,
                    account.to_string(),
                    worker.to_string(),
                    format!("/__workers/{account}/app/")
                ],
            )
            .unwrap();
    }
    (account, worker, route)
}

#[test]
fn localhost_worker_origin_migration_converts_routes_and_rejects_missing_authority() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("valid.sqlite");
    let (account, worker, route) = seed_v5(&path, true);
    let db = crate::ControlDb::open(&path, 5_000).unwrap();
    crate::migrations::apply(&db, &SystemClock).unwrap();
    db.with_read(|connection| {
        let migrated = connection
            .query_row(
                "SELECT c.id, c.hostname_ascii, r.worker_id, r.path_prefix
                 FROM hostname_claims c
                 JOIN worker_host_routes r ON r.claim_id = c.id",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            migrated,
            (
                route,
                format!("app.{account}.localhost"),
                worker.to_string(),
                "/".to_owned()
            )
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type='table' AND name='worker_routes'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        Ok(())
    })
    .unwrap();

    let invalid_path = temp.path().join("missing-route.sqlite");
    seed_v5(&invalid_path, false);
    let invalid = crate::ControlDb::open(&invalid_path, 5_000).unwrap();
    assert_eq!(
        crate::migrations::apply(&invalid, &SystemClock)
            .unwrap_err()
            .code(),
        ErrorCode::MigrationFailed
    );
}
