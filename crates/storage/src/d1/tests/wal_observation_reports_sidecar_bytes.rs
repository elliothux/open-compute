use super::*;

#[test]
fn wal_observation_reports_sidecar_bytes() {
    let fixture = fixture();
    fixture
        .engine
        .exec("CREATE TABLE wal_probe(value INTEGER)", limits())
        .unwrap();
    assert!(fixture.engine.wal_bytes().unwrap() <= fixture.engine.quota_bytes);
}
