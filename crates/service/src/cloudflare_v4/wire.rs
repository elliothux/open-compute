//! One v4 JSON envelope, authentication context, and sanitized error authority.

use crate::auth::bearer_matches;
use crate::http::{HttpState, REQUEST_ID_HEADER};
use axum::Json;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use open_compute_core::{ErrorCode, PlatformError, RequestId};
use serde::Serialize;

/// Permission role carried only in trusted request extensions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum V4Role {
    /// Installation administration, including maintenance and restore.
    Admin,
    /// Worker and explicitly authorized resource mutation.
    Deployer,
    /// Catalog and status inspection without mutation.
    ReadOnly,
}

/// Authenticated v4 request context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct V4RequestContext {
    /// Effective token role.
    pub(super) role: V4Role,
    /// Locally generated trace identifier.
    pub(super) request_id: RequestId,
}

impl V4RequestContext {
    /// Effective token role.
    pub(crate) const fn role(self) -> V4Role {
        self.role
    }

    /// Locally generated request identifier.
    pub(crate) const fn request_id(self) -> RequestId {
        self.request_id
    }

    /// Fail closed unless this role grants the endpoint permission.
    pub(crate) fn require(self, permission: V4Permission) -> Result<(), V4Error> {
        let granted = match self.role {
            V4Role::Admin => true,
            V4Role::Deployer => {
                matches!(permission, V4Permission::Read | V4Permission::ProductWrite)
            }
            V4Role::ReadOnly => matches!(permission, V4Permission::Read),
        };
        granted.then_some(()).ok_or(V4Error::PermissionDenied)
    }
}

/// Endpoint permission independent of the concrete token role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum V4Permission {
    /// Catalog, resource, and status reads.
    Read,
    /// Standard Worker or product resource mutations.
    #[allow(
        dead_code,
        reason = "P6 product subrouters consume this shared permission during integration"
    )]
    ProductWrite,
    /// Installation maintenance, backup, or restore.
    Maintenance,
}

/// Stable vendor error authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum V4Error {
    /// A registered error from an official Cloudflare endpoint contract.
    Official(V4OfficialError),
    /// Bearer token is absent or invalid.
    AuthenticationRequired,
    /// The authenticated role does not grant the operation.
    PermissionDenied,
    /// Request fields, query, path, or content type are invalid.
    InvalidRequest,
    /// A registered request field is invalid, with a stable JSON pointer.
    InvalidField(&'static str),
    /// The addressed account or resource does not exist.
    NotFound,
    /// Required local authority is not ready.
    Unavailable,
    /// Current state conflicts with the operation.
    Conflict,
    /// A persisted or downloaded artifact failed integrity validation.
    IntegrityFailure,
    /// The operation is outside the declared release capability.
    Unsupported,
    /// Local bounded admission is temporarily saturated.
    RateLimited,
    /// An internal operation failed without exposing its cause.
    Internal,
}

/// Registered official Cloudflare errors used by product-specific adapters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum V4OfficialError {
    /// Cloudflare common authentication failure.
    Authentication,
    /// A bounded request exceeds the endpoint maximum.
    RequestTooLarge,
}

impl V4Error {
    const fn code(self) -> u32 {
        match self {
            Self::Official(error) => error.code(),
            Self::AuthenticationRequired => 9_100_001,
            Self::PermissionDenied => 9_100_002,
            Self::InvalidRequest | Self::InvalidField(_) => 9_100_003,
            Self::NotFound => 9_100_004,
            Self::Unavailable => 9_100_005,
            Self::Conflict => 9_100_006,
            Self::IntegrityFailure => 9_100_009,
            Self::Unsupported => 9_100_007,
            Self::Internal => 9_100_008,
            Self::RateLimited => 9_102_001,
        }
    }

    const fn status(self) -> StatusCode {
        match self {
            Self::Official(error) => error.status(),
            Self::AuthenticationRequired => StatusCode::UNAUTHORIZED,
            Self::PermissionDenied => StatusCode::FORBIDDEN,
            Self::InvalidRequest | Self::InvalidField(_) => StatusCode::BAD_REQUEST,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::Conflict => StatusCode::CONFLICT,
            Self::IntegrityFailure => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Unsupported => StatusCode::NOT_IMPLEMENTED,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        }
    }

    const fn message(self) -> &'static str {
        match self {
            Self::Official(error) => error.message(),
            Self::AuthenticationRequired => "authentication is required",
            Self::PermissionDenied => "permission is denied",
            Self::InvalidRequest | Self::InvalidField(_) => "the request is invalid",
            Self::NotFound => "the requested resource was not found",
            Self::Unavailable => "the requested capability is unavailable",
            Self::Conflict => "the request conflicts with current state",
            Self::IntegrityFailure => "artifact integrity validation failed",
            Self::Unsupported => "the requested capability is not supported",
            Self::Internal => "the request could not be completed",
            Self::RateLimited => "local admission is temporarily saturated",
        }
    }

    const fn source_pointer(self) -> Option<&'static str> {
        match self {
            Self::InvalidField(source_pointer) => Some(source_pointer),
            _ => None,
        }
    }

    const fn retry_after_seconds(self) -> Option<u16> {
        match self {
            Self::Unavailable | Self::RateLimited => Some(1),
            _ => None,
        }
    }
}

impl V4OfficialError {
    const fn code(self) -> u32 {
        match self {
            Self::Authentication => 10_000,
            Self::RequestTooLarge => 10_027,
        }
    }

    const fn status(self) -> StatusCode {
        match self {
            Self::Authentication => StatusCode::UNAUTHORIZED,
            Self::RequestTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        }
    }

    const fn message(self) -> &'static str {
        match self {
            Self::Authentication => "Authentication error",
            Self::RequestTooLarge => "Request body is too large",
        }
    }
}

impl From<&PlatformError> for V4Error {
    fn from(error: &PlatformError) -> Self {
        match error.code() {
            ErrorCode::AdminAuthRequired => Self::AuthenticationRequired,
            ErrorCode::BundleTooLarge
            | ErrorCode::AssetLimitExceeded
            | ErrorCode::KvValueTooLarge
            | ErrorCode::R2ObjectTooLarge => Self::Official(V4OfficialError::RequestTooLarge),
            ErrorCode::QuotaExceeded
            | ErrorCode::AdmissionBusy
            | ErrorCode::KvBusy
            | ErrorCode::KvStorageFull
            | ErrorCode::R2Overloaded => Self::RateLimited,
            ErrorCode::AccountNotFound
            | ErrorCode::WorkerNotFound
            | ErrorCode::ResourceNotFound
            | ErrorCode::DoNamespaceNotFound => Self::NotFound,
            ErrorCode::PlatformUnavailable
            | ErrorCode::ResourceNotReady
            | ErrorCode::ResourceUnavailable
            | ErrorCode::KvUnavailable
            | ErrorCode::R2ProviderUnavailable => Self::Unavailable,
            ErrorCode::IdempotencyConflict
            | ErrorCode::RouteConflict
            | ErrorCode::WorkerNameConflict
            | ErrorCode::ResourceNameConflict
            | ErrorCode::R2BucketNotEmpty
            | ErrorCode::R2PreconditionFailed => Self::Conflict,
            ErrorCode::BindingCapabilityUnsupported | ErrorCode::CompatibilityUnsupported => {
                Self::Unsupported
            }
            ErrorCode::ConfigInvalid
            | ErrorCode::PathInvalid
            | ErrorCode::LimitInvalid
            | ErrorCode::KvKeyInvalid
            | ErrorCode::KvKeyTooLarge
            | ErrorCode::KvMetadataInvalid
            | ErrorCode::KvMetadataTooLarge
            | ErrorCode::KvInvalidOptions
            | ErrorCode::KvTooManyKeys
            | ErrorCode::KvCursorInvalid
            | ErrorCode::R2KeyTooLarge
            | ErrorCode::R2InvalidOptions
            | ErrorCode::R2ChecksumMismatch
            | ErrorCode::R2SsecInvalid
            | ErrorCode::R2MultipartInvalid
            | ErrorCode::R2MetadataTooLarge
            | ErrorCode::R2CursorInvalid => Self::InvalidRequest,
            _ => Self::Internal,
        }
    }
}

#[derive(Serialize)]
struct SuccessEnvelope<T> {
    success: bool,
    result: T,
    errors: [WireError; 0],
    messages: [WireMessage; 0],
}

#[derive(Serialize)]
struct PaginatedSuccessEnvelope<T> {
    success: bool,
    result: T,
    result_info: V4ResultInfo,
    errors: [WireError; 0],
    messages: [WireMessage; 0],
}

/// Official page/per-page collection metadata kept beside `result`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct V4ResultInfo {
    /// Current one-based page.
    pub(crate) page: usize,
    /// Requested maximum records per page.
    pub(crate) per_page: usize,
    /// Records returned on this page.
    pub(crate) count: usize,
    /// Records matching before pagination.
    pub(crate) total_count: usize,
}

#[derive(Serialize)]
struct ErrorEnvelope {
    success: bool,
    result: Option<()>,
    errors: [WireError; 1],
    messages: [WireMessage; 0],
}

#[derive(Clone, Serialize)]
struct WireError {
    code: u32,
    message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<WireErrorSource>,
}

#[derive(Clone, Serialize)]
struct WireErrorSource {
    pointer: &'static str,
}

#[derive(Clone, Serialize)]
struct WireMessage {
    code: u32,
    message: &'static str,
}

/// Wrap a typed successful result in the canonical JSON envelope.
pub(crate) fn success_response<T: Serialize>(context: V4RequestContext, result: T) -> Response {
    let mut response = Json(SuccessEnvelope {
        success: true,
        result,
        errors: [],
        messages: [],
    })
    .into_response();
    attach_request_id(&mut response, context.request_id);
    response
}

/// Wrap a typed page in the canonical envelope with sibling `result_info`.
pub(crate) fn paginated_response<T: Serialize>(
    context: V4RequestContext,
    result: T,
    result_info: V4ResultInfo,
) -> Response {
    let mut response = Json(PaginatedSuccessEnvelope {
        success: true,
        result,
        result_info,
        errors: [],
        messages: [],
    })
    .into_response();
    attach_request_id(&mut response, context.request_id);
    response
}

/// Return a sanitized non-2xx failure in the canonical JSON envelope.
pub(crate) fn error_response(error: V4Error, request_id: RequestId) -> Response {
    let mut response = (
        error.status(),
        Json(ErrorEnvelope {
            success: false,
            result: None,
            errors: [WireError {
                code: error.code(),
                message: error.message(),
                source: error
                    .source_pointer()
                    .map(|pointer| WireErrorSource { pointer }),
            }],
            messages: [],
        }),
    )
        .into_response();
    attach_request_id(&mut response, request_id);
    if let Some(seconds) = error.retry_after_seconds()
        && let Ok(value) = HeaderValue::from_str(&seconds.to_string())
    {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

/// Read the trusted context installed by the authentication boundary.
pub(crate) fn request_context(request: &Request) -> Result<V4RequestContext, Response> {
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .copied()
        .unwrap_or_else(RequestId::generate);
    request
        .extensions()
        .get::<V4RequestContext>()
        .copied()
        .ok_or_else(|| {
            error_response(
                V4Error::Official(V4OfficialError::Authentication),
                request_id,
            )
        })
}

pub(super) async fn authentication_boundary(
    State(state): State<HttpState>,
    mut request: Request,
    next: Next,
) -> Response {
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .copied()
        .unwrap_or_else(RequestId::generate);
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let matches = [
        (
            V4Role::Admin,
            state
                .admin_secret()
                .is_some_and(|secret| bearer_matches(presented, secret)),
        ),
        (
            V4Role::Deployer,
            state
                .deployer_secret()
                .is_some_and(|secret| bearer_matches(presented, secret)),
        ),
        (
            V4Role::ReadOnly,
            state
                .read_only_secret()
                .is_some_and(|secret| bearer_matches(presented, secret)),
        ),
    ];
    let mut roles = matches
        .into_iter()
        .filter_map(|(role, matched)| matched.then_some(role));
    let Some(role) = roles.next() else {
        return error_response(
            V4Error::Official(V4OfficialError::Authentication),
            request_id,
        );
    };
    if roles.next().is_some() {
        return error_response(
            V4Error::Official(V4OfficialError::Authentication),
            request_id,
        );
    }
    request
        .extensions_mut()
        .insert(V4RequestContext { role, request_id });
    next.run(request).await
}

fn attach_request_id(response: &mut Response, request_id: RequestId) {
    if let Ok(value) = HeaderValue::from_str(&request_id.to_string()) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
}

pub(super) async fn not_found(request: Request) -> Response {
    match request_context(&request) {
        Ok(context) => error_response(V4Error::NotFound, context.request_id),
        Err(response) => response,
    }
}
