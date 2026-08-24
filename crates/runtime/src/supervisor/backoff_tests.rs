use super::*;
use std::time::UNIX_EPOCH;

#[test]
fn exponential_caps_and_budget_stops() {
    let cfg = RuntimeConfig {
        restart_backoff_initial_ms: 10,
        restart_backoff_max_ms: 80,
        ..RuntimeConfig::default()
    };
    struct Zero;
    impl JitterRng for Zero {
        fn jitter(&self, _: u64) -> u64 {
            0
        }
    }
    assert_eq!(backoff_delay(&cfg, 1, &Zero).as_millis(), 10);
    assert_eq!(backoff_delay(&cfg, 2, &Zero).as_millis(), 20);
    assert_eq!(backoff_delay(&cfg, 4, &Zero).as_millis(), 80);
    let mut b = RestartBudget::new();
    for i in 0..3 {
        b.record(UNIX_EPOCH + Duration::from_secs(i), Duration::from_secs(60));
    }
    assert!(b.exceeded(3));
    assert!(!b.exceeded(4));
}

#[test]
fn zero_jitter_and_expired_restart_events_are_handled() {
    assert_eq!(OsJitter.jitter(0), 0);
    let mut budget = RestartBudget::new();
    budget.record(UNIX_EPOCH, Duration::from_secs(60));
    budget.prune(
        UNIX_EPOCH + Duration::from_secs(61),
        Duration::from_secs(60),
    );
    assert!(!budget.exceeded(1));
}
