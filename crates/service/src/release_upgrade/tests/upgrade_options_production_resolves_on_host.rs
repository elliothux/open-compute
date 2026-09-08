use super::*;

#[test]
fn upgrade_options_production_resolves_on_host() {
    let options = UpgradeOptions::production(None, true, true).unwrap();
    assert!(options.binary_path.is_absolute() || options.binary_path.exists());
    assert!(!options.target.is_empty());
    assert_eq!(host_release_target().unwrap(), options.target);
}
