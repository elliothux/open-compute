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
fn report_helpers_cover_all_statuses_bounds_and_write_failures() {
    let report = DoctorReport {
        schema_version: 1,
        command: "doctor",
        result: "failed",
        checks: vec![
            ok("ok", "ok", Some("value".to_owned())),
            warning("warning", "warning", None),
            failed("failed", ErrorCode::ConfigInvalid, "failed", None),
            skipped("skipped", "skipped"),
        ],
    };
    assert!(report.failed());
    let mut human = Vec::new();
    report.write(&mut human, false).unwrap();
    let human = String::from_utf8(human).unwrap();
    for status in ["ok", "warning", "failed", "skipped"] {
        assert!(human.contains(status));
    }
    let mut json = Vec::new();
    report.write(&mut json, true).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&json).unwrap()["result"],
        "failed"
    );
    assert_eq!(
        report.write(&mut RejectWrites, false).unwrap_err().code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        report.write(&mut RejectWrites, true).unwrap_err().code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(bound_value("aéz", 3), "aé");
    assert_eq!(bound_value("aéz", 2), "a");
}
