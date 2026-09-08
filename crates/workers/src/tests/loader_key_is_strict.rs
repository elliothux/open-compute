use super::*;

#[test]
fn loader_key_is_strict() {
    let account = AccountId::generate();
    let worker = WorkerId::generate();
    let version = VersionId::generate();
    let key = loader_key(account, worker, version);
    assert_eq!(parse_loader_key(&key).unwrap(), (account, worker, version));
    assert!(parse_loader_key(&format!("{key}/extra")).is_err());
    assert!(parse_loader_key(&key.replace('-', "%2d")).is_err());
    for invalid in ["", "a/b", "a/b/c", "a/b/c/d"] {
        assert_eq!(
            parse_loader_key(invalid).unwrap_err().code(),
            ErrorCode::VersionInvariantViolation
        );
    }
}
