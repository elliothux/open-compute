use super::*;
use std::time::UNIX_EPOCH;

#[test]
fn deterministic_clock_advances_only_when_asked() {
    let clock = DeterministicClock::new(UNIX_EPOCH);
    assert_eq!(clock.now(), UNIX_EPOCH);
    clock.advance(Duration::from_secs(5));
    assert_eq!(clock.now(), UNIX_EPOCH + Duration::from_secs(5));
    clock.set(UNIX_EPOCH + Duration::from_secs(90));
    assert_eq!(clock.now(), UNIX_EPOCH + Duration::from_secs(90));
}

#[test]
fn system_clock_is_at_or_after_unix_epoch() {
    assert!(SystemClock.now() >= UNIX_EPOCH);
}
