use super::*;

#[test]
fn atomic_replace_rename_failure_and_failing_writer() {
    let temp = TempDir::new().unwrap();
    let staged = temp.path().join("staged");
    write_staged_binary(&staged, b"#!/bin/sh\necho ok\n").unwrap();
    let dest_dir = temp.path().join("dest-as-dir");
    fs::create_dir(&dest_dir).unwrap();
    // Same parent, but rename onto a directory fails.
    let err = atomic_replace_binary(&staged, &dest_dir).unwrap_err();
    assert_eq!(err.code(), ErrorCode::PathInvalid);

    struct FailWrite;
    impl Write for FailWrite {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("nope"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let err = writeln!(FailWrite, "x")
        .map_err(|_| io_failed())
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::Internal);
    assert_eq!(err.message(), io_failed().message());
}
