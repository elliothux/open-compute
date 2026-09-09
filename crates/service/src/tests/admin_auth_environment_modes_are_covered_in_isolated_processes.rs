use super::*;

#[test]
fn admin_auth_environment_modes_are_covered_in_isolated_processes() {
    const MARKER: &str = "OPEN_COMPUTE_ADMIN_AUTH_CHILD_MODE";
    const SECRET_ENV: &str = "OPEN_COMPUTE_ADMIN_AUTH_CHILD_SECRET";
    if let Ok(mode) = std::env::var(MARKER) {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("admin-token");
        write_mode(
            &file,
            if mode == "mismatch" {
                "other"
            } else {
                "secret"
            },
            0o600,
        );
        let reference = SecretReference {
            env: Some(SECRET_ENV.to_owned()),
            file: if mode == "env-only" || mode == "empty" || mode == "large" {
                None
            } else {
                Some(file)
            },
        };
        let result = resolve_admin_auth(&reference);
        match mode.as_str() {
            "env-only" | "match" => assert_eq!(result.unwrap().expose(), "secret"),
            "mismatch" | "empty" | "large" => {
                assert_eq!(result.unwrap_err().code(), ErrorCode::SecretRefInvalid);
            }
            _ => panic!("unexpected child mode"),
        }
        return;
    }

    let current = std::env::current_exe().unwrap();
    for (mode, value) in [
        ("env-only", "secret".to_owned()),
        ("match", "secret".to_owned()),
        ("mismatch", "secret".to_owned()),
        ("empty", String::new()),
        ("large", "x".repeat(257)),
    ] {
        let status = Proc::new(&current)
            .args([
                "--exact",
                "tests::admin_auth_environment_modes_are_covered_in_isolated_processes::admin_auth_environment_modes_are_covered_in_isolated_processes",
                "--test-threads=1",
            ])
            .env(MARKER, mode)
            .env(SECRET_ENV, value)
            .status()
            .unwrap();
        assert!(status.success(), "child mode {mode} failed");
    }
}
