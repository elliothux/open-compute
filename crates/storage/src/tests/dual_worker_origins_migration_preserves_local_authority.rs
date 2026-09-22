use super::*;

fn seed_v7(path: &Path, corrupt: bool) -> (String, String, String) {
    let mut connection = Connection::open(path).unwrap();
    crate::schema_migrations::migrate_to_for_test(
        &mut connection,
        crate::schema_migrations::DatabaseKind::Control,
        7,
    );
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    let account = AccountId::generate().to_string();
    let worker = WorkerId::generate().to_string();
    let claim = uuid::Uuid::now_v7().to_string();
    connection
        .execute(
            "INSERT INTO accounts(id, name, created_at_ms, deleted_at_ms)
             VALUES(?1, 'dual-origin-account', 1, NULL)",
            [&account],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO workers
             (id, account_id, name, active_deployment_id, do_storage_id,
              route_generation, created_at_ms, updated_at_ms, deleted_at_ms, ownership)
             VALUES(?1, ?2, 'app', NULL, ?3, 1, 2, 2, NULL, 'tenant')",
            rusqlite::params![worker, account, uuid::Uuid::now_v7().to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO hostname_claims
             (id, hostname_ascii, account_id, namespace, exposure, state,
              generation, created_at_ms, updated_at_ms, deleted_at_ms)
             VALUES(?1, ?2, ?3, 'worker', 'local', 'active', 1, 3, 3, NULL)",
            rusqlite::params![claim, format!("app.{account}.localhost"), account],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO worker_host_routes
             (id, claim_id, account_id, worker_id, path_prefix, entrypoint,
              state, generation, created_at_ms, updated_at_ms, deleted_at_ms)
             VALUES(?1, ?1, ?2, ?3, '/', NULL, 'active', 1, 3, 3, NULL)",
            rusqlite::params![claim, account, worker],
        )
        .unwrap();
    if corrupt {
        connection
            .execute(
                "UPDATE worker_host_routes SET state = 'tombstoned', deleted_at_ms = 4",
                [],
            )
            .unwrap();
    }
    (account, worker, claim)
}

#[test]
fn dual_worker_origins_migration_preserves_local_authority() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("valid.sqlite");
    let (account, worker, claim) = seed_v7(&path, false);
    let db = crate::ControlDb::open(&path, 5_000).unwrap();
    crate::migrations::apply(&db, &SystemClock).unwrap();
    db.with_immediate(|tx| {
        let local: (String, String, String) = tx
            .query_row(
                "SELECT c.id, r.id, r.exposure FROM hostname_claims c
                 JOIN worker_host_routes r ON r.claim_id = c.id",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(local, (claim.clone(), claim.clone(), "local".to_owned()));

        assert!(
            tx.execute(
                "INSERT INTO hostname_claims
                 (id, hostname_ascii, account_id, namespace, exposure, state,
                  generation, created_at_ms, updated_at_ms, deleted_at_ms)
                 VALUES(?1, 'early.gateway-test.open-compute.dev', ?2, 'worker',
                        'public', 'active', 1, 5, 5, NULL)",
                rusqlite::params![uuid::Uuid::now_v7().to_string(), account],
            )
            .is_err()
        );

        tx.execute(
            "INSERT INTO public_gateway_domains
             (id, base_domain_ascii, state, generation, updated_at_ms)
             VALUES(1, 'gateway-test.open-compute.dev', 'active', 1, 5)",
            [],
        )
        .unwrap();

        tx.execute(
            "INSERT INTO public_gateway_namespaces
             (name, domain_id, state, generation, qualified_at_ms, updated_at_ms)
             VALUES('worker', 1, 'active', 1, 5, 5)",
            [],
        )
        .unwrap();
        assert!(
            tx.execute(
                "INSERT INTO hostname_claims
                 (id, hostname_ascii, account_id, namespace, exposure, state,
                  generation, created_at_ms, updated_at_ms, deleted_at_ms)
                 VALUES(?1, 'app.example.net', ?2, 'worker', 'public',
                        'active', 1, 5, 5, NULL)",
                rusqlite::params![uuid::Uuid::now_v7().to_string(), account],
            )
            .is_err()
        );

        for hostname in [
            "-bad.gateway-test.open-compute.dev",
            "bad-.gateway-test.open-compute.dev",
            "admin.gateway-test.open-compute.dev",
            "probe.gateway-test.open-compute.dev",
        ] {
            assert!(
                tx.execute(
                    "INSERT INTO hostname_claims
                     (id, hostname_ascii, account_id, namespace, exposure, state,
                      generation, created_at_ms, updated_at_ms, deleted_at_ms)
                     VALUES(?1, ?2, ?3, 'worker', 'public', 'active', 1, 5, 5, NULL)",
                    rusqlite::params![uuid::Uuid::now_v7().to_string(), hostname, account],
                )
                .is_err(),
                "accepted invalid public hostname: {hostname}"
            );
        }

        let public_claim = uuid::Uuid::now_v7().to_string();
        tx.execute(
            "INSERT INTO hostname_claims
             (id, hostname_ascii, account_id, namespace, exposure, state,
              generation, created_at_ms, updated_at_ms, deleted_at_ms)
             VALUES(?1, 'app.gateway-test.open-compute.dev', ?2, 'worker', 'public',
                    'active', 1, 5, 5, NULL)",
            rusqlite::params![public_claim, account],
        )
        .unwrap();
        assert!(
            tx.execute(
                "INSERT INTO worker_host_routes
             (id, claim_id, account_id, worker_id, namespace, exposure,
              path_prefix, entrypoint, state, generation, created_at_ms,
              updated_at_ms, deleted_at_ms)
             VALUES(?1, ?1, ?2, ?3, 'worker', 'local', '/', NULL,
                    'active', 1, 5, 5, NULL)",
                rusqlite::params![public_claim, account, worker],
            )
            .is_err()
        );
        tx.execute(
            "INSERT INTO worker_host_routes
             (id, claim_id, account_id, worker_id, namespace, exposure,
              path_prefix, entrypoint, state, generation, created_at_ms,
              updated_at_ms, deleted_at_ms)
             VALUES(?1, ?1, ?2, ?3, 'worker', 'public', '/', NULL,
                    'active', 1, 5, 5, NULL)",
            rusqlite::params![public_claim, account, worker],
        )
        .unwrap();
        assert!(
            tx.execute(
                "UPDATE public_gateway_domains SET base_domain_ascii = 'other.example.com'",
                [],
            )
            .is_err()
        );
        let second_claim = uuid::Uuid::now_v7().to_string();
        tx.execute(
            "INSERT INTO hostname_claims
             (id, hostname_ascii, account_id, namespace, exposure, state,
              generation, created_at_ms, updated_at_ms, deleted_at_ms)
             VALUES(?1, 'other.gateway-test.open-compute.dev', ?2, 'worker',
                    'public', 'active', 1, 6, 6, NULL)",
            rusqlite::params![second_claim, account],
        )
        .unwrap();
        assert!(
            tx.execute(
                "INSERT INTO worker_host_routes
             (id, claim_id, account_id, worker_id, namespace, exposure,
              path_prefix, entrypoint, state, generation, created_at_ms,
              updated_at_ms, deleted_at_ms)
             VALUES(?1, ?1, ?2, ?3, 'worker', 'public', '/', NULL,
                    'active', 1, 6, 6, NULL)",
                rusqlite::params![second_claim, account, worker],
            )
            .is_err()
        );
        Ok(())
    })
    .unwrap();
    assert!(
        db.with_immediate(|tx| {
            tx.execute(
                "UPDATE worker_host_routes
                 SET state = 'tombstoned', deleted_at_ms = 7
                 WHERE exposure = 'public' AND state = 'active'",
                [],
            )
            .unwrap();
            Ok(())
        })
        .is_err(),
        "route and claim state diverged at commit"
    );
    db.with_immediate(|tx| {
        tx.execute(
            "UPDATE worker_host_routes
             SET state = 'tombstoned', deleted_at_ms = 8
             WHERE exposure = 'public' AND state = 'active'",
            [],
        )
        .unwrap();
        tx.execute(
            "UPDATE hostname_claims
             SET state = 'tombstoned', deleted_at_ms = 8
             WHERE exposure = 'public' AND state = 'active'",
            [],
        )
        .unwrap();
        Ok(())
    })
    .unwrap();
    db.with_immediate(|tx| {
        assert!(
            tx.execute(
                "UPDATE worker_host_routes
                 SET state = 'active', deleted_at_ms = NULL
                 WHERE exposure = 'public' AND state = 'tombstoned'",
                [],
            )
            .is_err()
        );
        assert!(
            tx.execute(
                "UPDATE hostname_claims
                 SET state = 'active', deleted_at_ms = NULL
                 WHERE exposure = 'public' AND state = 'tombstoned'",
                [],
            )
            .is_err()
        );
        Ok(())
    })
    .unwrap();

    let corrupt_path = temp.path().join("corrupt.sqlite");
    seed_v7(&corrupt_path, true);
    let corrupt_db = crate::ControlDb::open(&corrupt_path, 5_000).unwrap();
    assert_eq!(
        crate::migrations::apply(&corrupt_db, &SystemClock)
            .unwrap_err()
            .code(),
        ErrorCode::MigrationFailed,
    );
}
