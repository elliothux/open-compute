use super::*;

#[test]
fn clean_and_repeat_bootstrap_preserves_identity() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let clock = DeterministicClock::new(UNIX_EPOCH + Duration::from_secs(1_700_000_000));
    let first = PlatformStorage::bootstrap(&config, &clock).expect("first");
    let platform_id = first.identity().platform_id;
    let account = first.identity().default_account_id;
    let created = first.identity().created_at_ms;
    drop(first);
    clock.advance(Duration::from_secs(60));
    let second = PlatformStorage::bootstrap(&config, &clock).expect("second");
    assert_eq!(second.identity().platform_id, platform_id);
    assert_eq!(second.identity().default_account_id, account);
    assert_eq!(second.identity().created_at_ms, created);
    assert_eq!(
        second
            .db()
            .query_meta("last_started_version")
            .unwrap()
            .as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
}
