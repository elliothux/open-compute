use super::*;

#[test]
fn package_manager_receipt_blocks_upgrade_options_path() {
    let temp = TempDir::new().unwrap();
    let binary_path = temp.path().join("opt/homebrew/bin/ocd");
    fs::create_dir_all(binary_path.parent().unwrap()).unwrap();
    // Force cellar-looking path component.
    let cellar = temp.path().join("opt/homebrew/Cellar/ocd/bin/ocd");
    fs::create_dir_all(cellar.parent().unwrap()).unwrap();
    fs::write(&cellar, b"x").unwrap();
    let err = require_upgradeable_receipt(&temp.path().join("missing.json"), &cellar).unwrap_err();
    assert!(err.message().contains("package-manager-owned"));
}
