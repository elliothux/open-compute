use super::*;

#[test]
fn p1_concurrent_resource_creates_never_exceed_the_account_kind_limit() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = Arc::new(PlatformStorage::bootstrap(&config, &SystemClock).unwrap());
    let account = storage.identity().default_account_id;
    let barrier = Arc::new(Barrier::new(9));
    let mut threads = Vec::new();
    for index in 0..8 {
        let storage = storage.clone();
        let barrier = barrier.clone();
        threads.push(thread::spawn(move || {
            let name = format!("p1-concurrent-{index}");
            let idempotency_key = format!("p1-concurrent-key-{index}");
            let fingerprint = storage.crypto().fingerprint_request(name.as_bytes());
            barrier.wait();
            ResourceRepository::new(storage.db()).reserve_create(
                &ReserveResourceCreate {
                    account_id: account,
                    kind: BindingKind::KvNamespace,
                    name: &name,
                    idempotency_key: &idempotency_key,
                    fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                    request_fingerprint: &fingerprint,
                    resource_id: ResourceId::generate(),
                    driver_schema_version: 1,
                    request_id: open_compute_core::RequestId::generate(),
                    now_ms: 1,
                    expires_at_ms: 1_001,
                },
                3,
            )
        }));
    }
    barrier.wait();
    let mut accepted = 0;
    let mut rejected = 0;
    for thread in threads {
        match thread.join().unwrap() {
            Ok(ResourceCreateReservation::Reserved(_)) => accepted += 1,
            Err(error) if error.code() == ErrorCode::QuotaExceeded => rejected += 1,
            other => panic!("unexpected concurrent resource result: {other:?}"),
        }
    }
    assert_eq!(accepted, 3);
    assert_eq!(rejected, 5);
    assert_eq!(
        ResourceRepository::new(storage.db())
            .list(account, Some(BindingKind::KvNamespace))
            .unwrap()
            .len(),
        3
    );
}
