use super::*;

#[test]
fn target_name_accepts_only_the_day1_grammar() {
    for valid in [
        "a",
        "dev-1",
        "company-prod",
        "a1234567890123456789012345678901",
    ] {
        assert_eq!(valid.parse::<TargetName>().unwrap().as_str(), valid);
    }
    for invalid in [
        "",
        "1dev",
        "Dev",
        "dev_1",
        "dev.",
        "a12345678901234567890123456789012",
    ] {
        assert!(invalid.parse::<TargetName>().is_err(), "{invalid}");
    }
}

#[test]
fn account_id_is_canonical_lowercase_hex() {
    let valid = "0123456789abcdef0123456789abcdef";
    assert_eq!(
        valid.parse::<CloudflareAccountId>().unwrap().as_str(),
        valid
    );
    for invalid in [
        "0123456789abcdef",
        "0123456789ABCDEF0123456789ABCDEF",
        "g123456789abcdef0123456789abcdef",
    ] {
        assert!(invalid.parse::<CloudflareAccountId>().is_err());
    }
}

#[test]
fn api_base_url_normalizes_only_the_supported_shape() {
    let https: TargetApiBaseUrl = "https://example.com/client/v4/".parse().unwrap();
    assert_eq!(https.as_str(), "https://example.com/client/v4");
    assert_eq!(https.origin(), "https://example.com");
    assert_eq!(
        https.endpoint("/accounts"),
        "https://example.com/client/v4/accounts"
    );
    assert!(
        "http://127.0.0.1:8787/client/v4"
            .parse::<TargetApiBaseUrl>()
            .is_ok()
    );
    assert!(
        "http://[::1]:8787/client/v4"
            .parse::<TargetApiBaseUrl>()
            .is_ok()
    );
    assert!(
        "http://localhost:8787/client/v4"
            .parse::<TargetApiBaseUrl>()
            .is_ok()
    );
    for invalid in [
        "http://example.com/client/v4",
        "https://user@example.com/client/v4",
        "https://example.com/client/v4?x=1",
        "https://example.com/client/v4#x",
        "https://example.com/client/v4/accounts",
    ] {
        assert!(invalid.parse::<TargetApiBaseUrl>().is_err(), "{invalid}");
    }
}
