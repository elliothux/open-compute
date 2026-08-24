//! Secret-safe S3 failure classification.

use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::delete_object::DeleteObjectError;
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_s3::operation::head_object::HeadObjectError;
use aws_sdk_s3::operation::list_objects_v2::ListObjectsV2Error;
use aws_sdk_s3::operation::put_object::PutObjectError;
use aws_smithy_runtime_api::client::orchestrator::HttpResponse;
use aws_smithy_runtime_api::client::result::ConnectorError;
use open_compute_core::{ErrorCode, PlatformError};
use std::fmt::{Debug, Display, Formatter};

/// Stage of an S3 operation that failed. Contains no keys, URLs, or secrets.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum S3Stage {
    /// DNS resolution failed.
    Dns,
    /// TLS handshake or certificate verification failed.
    Tls,
    /// Access key was rejected.
    Auth,
    /// Request signature was rejected.
    Signature,
    /// Region mismatch or permanent redirect.
    Region,
    /// Bucket does not exist or is not reachable as configured.
    Bucket,
    /// Bucket policy / IAM denied the operation.
    Policy,
    /// Connect or request timeout.
    Timeout,
    /// 5xx or other retry-exhausted server failure.
    Server,
    /// Object delete failed or post-delete visibility was wrong.
    Delete,
    /// Remote bytes or metadata failed integrity checks.
    Integrity,
    /// Object was not found.
    NotFound,
}

impl S3Stage {
    /// Canonical operator token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dns => "DNS",
            Self::Tls => "TLS",
            Self::Auth => "AUTH",
            Self::Signature => "SIGNATURE",
            Self::Region => "REGION",
            Self::Bucket => "BUCKET",
            Self::Policy => "POLICY",
            Self::Timeout => "TIMEOUT",
            Self::Server => "SERVER",
            Self::Delete => "DELETE",
            Self::Integrity => "INTEGRITY",
            Self::NotFound => "NOT_FOUND",
        }
    }
}

impl Display for S3Stage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Classified S3 failure that never includes credentials, keys, or signed URLs.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct S3Failure {
    code: ErrorCode,
    stage: S3Stage,
}

impl S3Failure {
    /// Construct a classified failure.
    #[must_use]
    pub const fn new(code: ErrorCode, stage: S3Stage) -> Self {
        Self { code, stage }
    }

    /// Stable platform error code.
    #[must_use]
    pub const fn code(self) -> ErrorCode {
        self.code
    }

    /// Failure stage.
    #[must_use]
    pub const fn stage(self) -> S3Stage {
        self.stage
    }

    /// Convert to a secret-safe [`PlatformError`].
    #[must_use]
    pub const fn to_platform_error(self) -> PlatformError {
        PlatformError::new(self.code, self.operator_message())
    }

    const fn operator_message(self) -> &'static str {
        match self.stage {
            S3Stage::Dns => "s3 dns resolution failed",
            S3Stage::Tls => "s3 tls verification failed",
            S3Stage::Auth => "s3 authentication failed",
            S3Stage::Signature => "s3 request signature was rejected",
            S3Stage::Region => "s3 region mismatch",
            S3Stage::Bucket => "s3 bucket is unavailable",
            S3Stage::Policy => "s3 access was denied by policy",
            S3Stage::Timeout => "s3 request timed out",
            S3Stage::Server => "s3 returned a server error",
            S3Stage::Delete => "s3 object delete failed",
            S3Stage::Integrity => "s3 object failed integrity verification",
            S3Stage::NotFound => "s3 object was not found",
        }
    }
}

impl Debug for S3Failure {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Failure")
            .field("code", &self.code.as_str())
            .field("stage", &self.stage.as_str())
            .finish()
    }
}

impl Display for S3Failure {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.stage.as_str())
    }
}

impl From<S3Failure> for PlatformError {
    fn from(value: S3Failure) -> Self {
        value.to_platform_error()
    }
}

pub(crate) fn integrity_error() -> PlatformError {
    S3Failure::new(ErrorCode::ArtifactIntegrityError, S3Stage::Integrity).into()
}

pub(crate) fn unavailable(stage: S3Stage) -> PlatformError {
    let code = if stage == S3Stage::Integrity {
        ErrorCode::ArtifactIntegrityError
    } else {
        ErrorCode::S3Unavailable
    };
    S3Failure::new(code, stage).into()
}

pub(crate) fn classify_http_status(status: u16, op: OpKind) -> S3Stage {
    match status {
        404 => {
            if op == OpKind::Delete {
                S3Stage::Delete
            } else {
                S3Stage::NotFound
            }
        }
        301 | 307 | 400 => S3Stage::Region,
        403 => S3Stage::Policy,
        401 => S3Stage::Auth,
        408 | 504 => S3Stage::Timeout,
        s if (500..600).contains(&s) => S3Stage::Server,
        _ => S3Stage::Server,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OpKind {
    Put,
    Head,
    Get,
    Delete,
    List,
}

pub(crate) fn classify_connector(err: &ConnectorError) -> S3Stage {
    let mut msg = err.to_string().to_ascii_lowercase();
    let mut source = std::error::Error::source(err);
    while let Some(next) = source {
        msg.push(' ');
        msg.push_str(&next.to_string().to_ascii_lowercase());
        source = next.source();
    }
    if msg.contains("dns") || msg.contains("failed to lookup") || msg.contains("name or service") {
        return S3Stage::Dns;
    }
    if msg.contains("tls") || msg.contains("certificate") || msg.contains("ssl") {
        return S3Stage::Tls;
    }
    if msg.contains("timed out") || msg.contains("timeout") {
        return S3Stage::Timeout;
    }
    S3Stage::Server
}

fn classify_sdk<E>(err: &SdkError<E, HttpResponse>, op: OpKind) -> S3Stage
where
    E: std::error::Error,
{
    match err {
        SdkError::TimeoutError(_) => S3Stage::Timeout,
        SdkError::DispatchFailure(disp) => {
            if let Some(conn) = disp.as_connector_error() {
                classify_connector(conn)
            } else {
                S3Stage::Server
            }
        }
        SdkError::ResponseError(resp) => classify_http_status(resp.raw().status().as_u16(), op),
        SdkError::ServiceError(svc) => {
            let status = svc.raw().status().as_u16();
            let code = svc.err().to_string();
            classify_service_code(&code, status, op)
        }
        SdkError::ConstructionFailure(_) => S3Stage::Server,
        _ => S3Stage::Server,
    }
}

pub(crate) fn classify_service_code(code: &str, status: u16, op: OpKind) -> S3Stage {
    let lower = code.to_ascii_lowercase();
    if lower.contains("invalidaccesskey") || lower.contains("invalidclienttoken") {
        return S3Stage::Auth;
    }
    if lower.contains("signature") {
        return S3Stage::Signature;
    }
    if lower.contains("accessdenied") || lower.contains("allaccessdisabled") {
        return S3Stage::Policy;
    }
    if lower.contains("nosuchbucket") || lower.contains("permanentredirect") {
        return S3Stage::Bucket;
    }
    if lower.contains("authorizationheadermalformed") || lower.contains("illegal location") {
        return S3Stage::Region;
    }
    if lower.contains("nosuchkey") || lower.contains("not found") {
        return if op == OpKind::Delete {
            S3Stage::Delete
        } else {
            S3Stage::NotFound
        };
    }
    classify_http_status(status, op)
}

pub(crate) fn from_put(err: &SdkError<PutObjectError, HttpResponse>) -> PlatformError {
    unavailable(classify_sdk(err, OpKind::Put))
}

pub(crate) fn from_head(err: &SdkError<HeadObjectError, HttpResponse>) -> PlatformError {
    unavailable(classify_sdk(err, OpKind::Head))
}

pub(crate) fn from_get(err: &SdkError<GetObjectError, HttpResponse>) -> PlatformError {
    unavailable(classify_sdk(err, OpKind::Get))
}

pub(crate) fn from_delete(err: &SdkError<DeleteObjectError, HttpResponse>) -> PlatformError {
    let stage = classify_sdk(err, OpKind::Delete);
    match stage {
        S3Stage::Timeout | S3Stage::Dns | S3Stage::Tls | S3Stage::Auth | S3Stage::Signature => {
            unavailable(stage)
        }
        _ => unavailable(S3Stage::Delete),
    }
}

pub(crate) fn from_list(err: &SdkError<ListObjectsV2Error, HttpResponse>) -> PlatformError {
    unavailable(classify_sdk(err, OpKind::List))
}

pub(crate) fn is_not_found(err: &PlatformError) -> bool {
    err.message() == S3Failure::new(ErrorCode::S3Unavailable, S3Stage::NotFound).operator_message()
}
