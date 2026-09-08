use super::*;

#[test]
fn config_path_boundary_helpers_reject_ambiguous_roots() {
    let temporary = TempDir::new().unwrap();
    assert_eq!(
        load_platform_config_from(Path::new("config.toml"), Path::new("relative-startup"))
            .unwrap_err()
            .code(),
        ErrorCode::ConfigPathInvalid
    );
    assert_eq!(
        load_platform_config_from(Path::new("/"), temporary.path())
            .unwrap_err()
            .code(),
        ErrorCode::ConfigPathInvalid
    );
    assert_eq!(
        load_platform_config_from(Path::new("missing/config.toml"), temporary.path())
            .unwrap_err()
            .code(),
        ErrorCode::ConfigPathInvalid
    );
    assert_eq!(
        crate::config_load::lexical_absolute(Path::new("/"), Path::new("../../config.toml"))
            .unwrap_err()
            .code(),
        ErrorCode::ConfigPathInvalid
    );
    assert_eq!(
        crate::config_load::lexical_absolute(
            Path::new("relative-startup"),
            Path::new("config.toml")
        )
        .unwrap_err()
        .code(),
        ErrorCode::ConfigPathInvalid
    );
}
