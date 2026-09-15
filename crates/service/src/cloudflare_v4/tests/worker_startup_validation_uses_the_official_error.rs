use super::*;
use open_compute_core::{ErrorCode, PlatformError, RequestId};

#[tokio::test]
async fn worker_startup_validation_uses_the_official_error() {
    let response = error_response(
        V4Error::from(&PlatformError::new(
            ErrorCode::BundleRuntimeInvalid,
            "private validator detail",
        )),
        RequestId::generate(),
    );
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json(response).await;
    assert_eq!(body["errors"][0]["code"], 10_021);
    assert_eq!(body["errors"][0]["message"], "Worker validation failed");
    assert!(!body.to_string().contains("private validator detail"));
}
