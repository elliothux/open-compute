use super::*;

#[tokio::test]
async fn config_check_has_no_side_effects() {
    let dir = TempDir::new().unwrap();
    let path = write_config(dir.path(), "");
    let before = snapshot(dir.path());
    let loaded = load_fixture_platform_config(&path);
    MetricsRegistry::validate_limits(&loaded.config.metrics).unwrap();
    let after = snapshot(dir.path());
    assert_eq!(before, after);
}
