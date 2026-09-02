use super::*;

struct RejectWrites;

impl Write for RejectWrites {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("rejected"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn embedded_resources_list_current_runbooks_and_report_output_failures() {
    let mut names = Vec::new();
    write_docs(None, &mut names).unwrap();
    let names = String::from_utf8(names).unwrap();
    assert!(names.contains("scheduler-recovery"));
    assert!(names.contains("workerd-crash-loop"));
    let mut licenses = Vec::new();
    write_licenses(&mut licenses).unwrap();
    let licenses = String::from_utf8(licenses).unwrap();
    assert!(licenses.contains("Embedded Xberg document parser"));
    assert!(licenses.contains("Copyright (c) 2025-2026 Kreuzberg, Inc."));
    assert_eq!(
        write_docs(None, &mut RejectWrites).unwrap_err().code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        write_licenses(&mut RejectWrites).unwrap_err().code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        write_config(Path::new("/tmp/open-compute-test"), &mut RejectWrites)
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );
}
