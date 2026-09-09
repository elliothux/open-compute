use super::*;

#[test]
fn upgrade_options_production_and_load_receipt() {
    let options = UpgradeOptions::production(None, true, true).unwrap();
    assert!(options.binary_path.is_absolute() || options.binary_path.exists());
    assert!(options.dry_run);
    assert!(options.no_restart);
    let _ = load_receipt_for_exe();
    let _ = LiveReleaseHttp::new().unwrap();
}
