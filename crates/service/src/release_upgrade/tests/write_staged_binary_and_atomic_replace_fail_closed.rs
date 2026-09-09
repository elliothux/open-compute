use super::*;

#[test]
fn write_staged_binary_and_atomic_replace_fail_closed() {
    let temp = TempDir::new().unwrap();
    let relative = Path::new("relative-stage");
    assert_eq!(
        write_staged_binary(relative, b"abc").unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    let staged = temp.path().join("staged");
    write_staged_binary(&staged, b"#!/bin/sh\necho ok\n").unwrap();
    assert!(staged.is_file());
    // create_new refuses overwrite.
    assert_eq!(
        write_staged_binary(&staged, b"again").unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    let other_dir = TempDir::new().unwrap();
    let other = other_dir.path().join("elsewhere");
    fs::write(&other, b"x").unwrap();
    assert!(
        atomic_replace_binary(&staged, &other)
            .unwrap_err()
            .message()
            .contains("same filesystem")
    );
}
