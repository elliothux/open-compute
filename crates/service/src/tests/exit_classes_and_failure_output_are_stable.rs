use super::*;

#[test]
fn exit_classes_and_failure_output_are_stable() {
    for code in [
        ErrorCode::ConfigPathInvalid,
        ErrorCode::ConfigParseFailed,
        ErrorCode::AdminAuthRequired,
        ErrorCode::SecretRefInvalid,
        ErrorCode::PathInvalid,
        ErrorCode::ObjectStoragePrefixInvalid,
        ErrorCode::CacheBoundsInvalid,
        ErrorCode::LimitInvalid,
    ] {
        assert_eq!(exit_class_for(code), ExitClass::Config);
    }
    assert_eq!(exit_class_for(ErrorCode::Internal), ExitClass::Run);
    assert_eq!(
        std::process::ExitCode::from(ExitClass::Cli),
        std::process::ExitCode::from(2)
    );
    let mut output = Vec::new();
    emit_failure(
        &open_compute_core::PlatformError::new(ErrorCode::Internal, "safe"),
        &mut output,
    )
    .unwrap();
    assert_eq!(output, b"INTERNAL: safe\n");
}
