use super::*;

#[test]
fn parse_sha256sums_skips_blank_lines_and_rejects_empty_fields() {
    let digest = "ab".repeat(32);
    let body = format!("\n  \n{digest}  file.bin\n");
    let map = parse_sha256sums(body.as_bytes()).unwrap();
    assert_eq!(
        map.get("file.bin").map(String::as_str),
        Some(digest.as_str())
    );
    let err = parse_sha256sums(b"onlydigest\n").unwrap_err();
    assert_eq!(err.code(), ErrorCode::ReleaseUnsupported);
}
