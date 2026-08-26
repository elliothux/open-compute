use super::*;

fn snapshot() -> AdmissionSnapshotV1 {
    AdmissionSnapshotV1 {
        schema_version: 1,
        filesystem_free_bytes: 10_000,
        soft_reserve_bytes: 2_000,
        hard_reserve_bytes: 1_000,
        emergency_reserve_bytes: 100,
        reserved_bytes: 500,
        owned_staging_bytes: 250,
        mode: PlatformMode::Serving,
    }
}

#[test]
fn admission_snapshot_and_reservations_fail_closed() {
    assert_eq!(snapshot().admit(250).unwrap(), 8_000);
    let mut pressure = snapshot();
    pressure.filesystem_free_bytes = 1_999;
    assert_eq!(
        pressure.admit(250).unwrap_err().code(),
        ErrorCode::StoragePressure
    );
    let mut draining = snapshot();
    draining.mode = PlatformMode::Draining;
    assert_eq!(
        draining.admit(1).unwrap_err().code(),
        ErrorCode::PlatformUnavailable
    );
    let mut invalid = snapshot();
    invalid.emergency_reserve_bytes = invalid.hard_reserve_bytes;
    assert_eq!(
        invalid.admit(1).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );

    let reservations = AdmissionReservations::default();
    let first = reservations.reserve(9).unwrap();
    assert_eq!(reservations.bytes(), 9);
    {
        let mut second = reservations.reserve(3).unwrap();
        assert_eq!(reservations.bytes(), 12);
        second.release();
        second.release();
    }
    assert_eq!(reservations.bytes(), 9);
    drop(first);
    assert_eq!(reservations.bytes(), 0);

    let full = reservations.reserve(u64::MAX).unwrap();
    assert_eq!(
        reservations.reserve(1).unwrap_err().code(),
        ErrorCode::AdmissionBusy
    );
    drop(full);
    assert_eq!(reservations.bytes(), 0);
}
