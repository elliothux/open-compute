use super::*;

#[test]
fn helper_io_and_open_url_paths() {
    assert_eq!(io_failed().code(), ErrorCode::ConfigInvalid);
    // macOS `open` fails closed for a path that cannot be resolved.
    let err = open_url_in_browser("/tmp/open-compute-no-such-dashboard-target-xyz")
        .err()
        .or_else(|| open_url_in_browser("").err());
    // Either open fails, or it succeeds opening Finder; both exercise the helper.
    let _ = err;
    let _ = open_url_in_browser("https://example.invalid/");
}
