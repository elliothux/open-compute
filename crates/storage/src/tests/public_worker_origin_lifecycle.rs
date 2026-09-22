use super::*;
use crate::{PublicGatewayRepository, WorkerOriginExposure};
use open_compute_core::{PublicGatewayConfig, RequestId};

fn gateway_config(base_domain: &str) -> PublicGatewayConfig {
    PublicGatewayConfig {
        base_domain: base_domain.to_owned(),
        ingress_ipv4: vec!["203.0.113.10".parse().unwrap()],
        ingress_ipv6: Vec::new(),
        https_listen: "127.0.0.1:8443".parse().unwrap(),
        challenge_dns_listen: "127.0.0.1:8053".parse().unwrap(),
        proxy_protocol_from: Vec::new(),
        caddy: Vec::new(),
    }
}

#[test]
fn public_worker_origin_lifecycle_preserves_local_and_survives_restart() {
    let (_temp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let workers = WorkerRepository::new(storage.db());
    let gateway = PublicGatewayRepository::new(storage.db());
    let request = RequestId::generate();
    let (worker, local) = workers
        .create_worker(account, "app", request, 1, 100)
        .unwrap();

    gateway
        .provision(&gateway_config("gateway-test.open-compute.dev"), 2)
        .unwrap();
    assert_eq!(
        workers
            .set_public_origin(account, worker.id, Some("app"), request, 3)
            .unwrap_err()
            .code(),
        ErrorCode::RouteConflict
    );
    gateway
        .activate_workers("gateway-test.open-compute.dev", 4)
        .unwrap();
    assert_eq!(
        workers
            .set_public_origin(account, worker.id, Some("probe"), request, 5)
            .unwrap_err()
            .code(),
        ErrorCode::RouteConflict
    );
    let first = workers
        .set_public_origin(account, worker.id, Some("app"), request, 5)
        .unwrap()
        .unwrap();
    assert_eq!(first.exposure, WorkerOriginExposure::Public);
    assert_eq!(first.hostname_ascii, "app.gateway-test.open-compute.dev");
    assert_eq!(
        workers
            .set_public_origin(account, worker.id, Some("app"), request, 6)
            .unwrap()
            .unwrap()
            .id,
        first.id
    );
    let (other, _) = workers
        .create_worker(account, "other", request, 7, 100)
        .unwrap();
    assert_eq!(
        workers
            .set_public_origin(account, other.id, Some("app"), request, 8)
            .unwrap_err()
            .code(),
        ErrorCode::RouteConflict
    );
    let replacement = workers
        .set_public_origin(account, worker.id, Some("next"), request, 9)
        .unwrap()
        .unwrap();
    assert_ne!(first.id, replacement.id);
    let routes = workers.list_routes(account, worker.id).unwrap();
    assert_eq!(routes.len(), 2);
    assert!(routes.iter().any(|route| route.id == local.id));
    assert!(routes.iter().any(|route| route.id == replacement.id));
    assert_eq!(
        gateway
            .provision(&gateway_config("other.example.com"), 10)
            .unwrap_err()
            .code(),
        ErrorCode::RouteConflict
    );
    assert_eq!(
        gateway.disable(10).unwrap_err().code(),
        ErrorCode::RouteConflict
    );

    workers
        .set_public_origin(account, worker.id, None, request, 11)
        .unwrap();
    assert_eq!(workers.list_routes(account, worker.id).unwrap().len(), 1);
    gateway.disable(12).unwrap();
    gateway.disable(12).unwrap();
    assert_eq!(format!("{gateway:?}"), "PublicGatewayRepository");
    assert_eq!(
        gateway
            .activate_workers("gateway-test.open-compute.dev", 13)
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        workers
            .set_public_origin(account, worker.id, Some("app"), request, 13)
            .unwrap_err()
            .code(),
        ErrorCode::RouteConflict
    );
    gateway
        .provision(&gateway_config("gateway-test.open-compute.dev"), 14)
        .unwrap();
    assert_eq!(
        workers
            .set_public_origin(account, worker.id, Some("app"), request, 14)
            .unwrap_err()
            .code(),
        ErrorCode::RouteConflict
    );
    gateway
        .provision(&gateway_config("other.example.com"), 15)
        .unwrap();
    gateway.activate_workers("other.example.com", 16).unwrap();
    let new_origin = workers
        .set_public_origin(account, worker.id, Some("app"), request, 17)
        .unwrap()
        .unwrap();
    assert_eq!(new_origin.hostname_ascii, "app.other.example.com");
    drop(storage);

    let restored = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let routes = WorkerRepository::new(restored.db())
        .list_routes(account, worker.id)
        .unwrap();
    assert_eq!(routes.len(), 2);
    assert!(routes.iter().any(|route| route.id == local.id));
    assert!(routes.iter().any(|route| route.id == new_origin.id));
}

#[test]
fn gateway_activation_rejects_missing_namespace_and_unknown_worker() {
    let (_temp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let gateway = PublicGatewayRepository::new(storage.db());
    gateway
        .provision(&gateway_config("compute.example.com"), 1)
        .unwrap();
    storage
        .db()
        .with_immediate(|tx| {
            tx.execute("DELETE FROM public_gateway_namespaces", [])
                .unwrap();
            Ok(())
        })
        .unwrap();
    assert_eq!(
        gateway
            .activate_workers("compute.example.com", 2)
            .unwrap_err()
            .code(),
        ErrorCode::VersionInvariantViolation
    );

    let workers = WorkerRepository::new(storage.db());
    assert_eq!(
        workers
            .set_public_origin(
                storage.identity().default_account_id,
                WorkerId::generate(),
                None,
                RequestId::generate(),
                3,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkerNotFound
    );
}

#[test]
fn public_origin_replacement_tombstones_the_joined_claim() {
    let (_temp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let workers = WorkerRepository::new(storage.db());
    let gateway = PublicGatewayRepository::new(storage.db());
    gateway
        .provision(&gateway_config("compute.example.com"), 1)
        .unwrap();
    gateway.activate_workers("compute.example.com", 2).unwrap();
    let request = RequestId::generate();
    let worker = workers
        .create_worker(account, "app", request, 3, 100)
        .unwrap()
        .0;
    let first = workers
        .set_public_origin(account, worker.id, Some("first"), request, 4)
        .unwrap()
        .unwrap();
    let route_id = uuid::Uuid::now_v7().to_string();
    storage
        .db()
        .with_immediate(|tx| {
            tx.execute(
                "UPDATE worker_host_routes SET id = ?1 WHERE id = ?2",
                [&route_id, &first.id],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();

    workers
        .set_public_origin(account, worker.id, Some("second"), request, 5)
        .unwrap();
    let (old_state, active_claims): (String, i64) = storage
        .db()
        .with_immediate(|tx| {
            let state = tx
                .query_row(
                    "SELECT state FROM hostname_claims WHERE id = ?1",
                    [&first.id],
                    |row| row.get(0),
                )
                .unwrap();
            let count = tx.query_row(
            "SELECT COUNT(*) FROM hostname_claims WHERE state = 'active' AND exposure = 'public'",
            [],
            |row| row.get(0),
        ).unwrap();
            Ok((state, count))
        })
        .unwrap();
    assert_eq!(old_state, "tombstoned");
    assert_eq!(active_claims, 1);
}

#[test]
fn gateway_restart_rejects_missing_worker_namespace_authority() {
    let (_temp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let gateway = PublicGatewayRepository::new(storage.db());
    let config = gateway_config("compute.example.com");
    gateway.provision(&config, 1).unwrap();
    storage
        .db()
        .with_immediate(|tx| {
            tx.execute(
                "DELETE FROM public_gateway_namespaces WHERE name = 'worker'",
                [],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    assert_eq!(
        gateway.provision(&config, 2).unwrap_err().code(),
        ErrorCode::VersionInvariantViolation
    );
    assert_eq!(
        gateway.disable(2).unwrap_err().code(),
        ErrorCode::VersionInvariantViolation
    );
}

#[test]
fn disabled_gateway_rejects_inconsistent_namespace_authority() {
    let (_temp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let gateway = PublicGatewayRepository::new(storage.db());
    let config = gateway_config("compute.example.com");
    gateway.provision(&config, 1).unwrap();
    gateway.disable(2).unwrap();
    storage
        .db()
        .with_immediate(|tx| {
            tx.execute(
                "UPDATE public_gateway_namespaces SET state = 'provisioning'
                 WHERE name = 'worker'",
                [],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    assert_eq!(
        gateway.provision(&config, 3).unwrap_err().code(),
        ErrorCode::VersionInvariantViolation
    );
    assert_eq!(
        gateway.disable(3).unwrap_err().code(),
        ErrorCode::VersionInvariantViolation
    );
}

#[test]
fn gateway_restart_rejects_invalid_persisted_domain() {
    let (_temp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let gateway = PublicGatewayRepository::new(storage.db());
    let config = gateway_config("compute.example.com");
    gateway.provision(&config, 1).unwrap();
    storage
        .db()
        .with_immediate(|tx| {
            tx.execute(
                "UPDATE public_gateway_domains SET base_domain_ascii = '-bad.example.com'",
                [],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    assert_eq!(
        gateway.provision(&config, 2).unwrap_err().code(),
        ErrorCode::VersionInvariantViolation
    );
}
