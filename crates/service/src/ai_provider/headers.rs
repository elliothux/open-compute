//! Provider metadata and request-scoped credentials share one header authority.

use super::AiProviderError;
use crate::auth::resolve_admin_auth;
use hyper::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use open_compute_core::{AiAuthConfig, AiBackendConfig};

pub(crate) fn request_authorization_token(value: &str) -> Result<&str, AiProviderError> {
    let token = value
        .split_once(' ')
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map_or(value, |(_, token)| token);
    if value.len() > 4096 || token.is_empty() || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(AiProviderError::InvalidRequest);
    }
    Ok(token)
}

pub(super) fn resolve_backend_headers(
    backend: &AiBackendConfig,
    authorization: Option<&str>,
) -> Result<HeaderMap, AiProviderError> {
    let mut headers = HeaderMap::new();
    for (name, value) in &backend.headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| AiProviderError::ContractMismatch)?;
        let value = HeaderValue::from_str(value).map_err(|_| AiProviderError::ContractMismatch)?;
        headers.insert(name, value);
    }
    if let Some(authorization) = authorization {
        let token = request_authorization_token(authorization)?;
        let (name, value) = match &backend.auth {
            AiAuthConfig::Header { name, .. } => (
                HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| AiProviderError::ContractMismatch)?,
                token.to_owned(),
            ),
            AiAuthConfig::Bearer { .. } | AiAuthConfig::None => {
                (AUTHORIZATION, format!("Bearer {token}"))
            }
        };
        let mut value =
            HeaderValue::from_str(&value).map_err(|_| AiProviderError::InvalidRequest)?;
        value.set_sensitive(true);
        headers.insert(name, value);
        return Ok(headers);
    }
    match &backend.auth {
        AiAuthConfig::None => {}
        AiAuthConfig::Bearer { secret } => {
            let secret =
                resolve_admin_auth(secret).map_err(|_| AiProviderError::ContractMismatch)?;
            let mut value = HeaderValue::from_str(&format!("Bearer {}", secret.expose()))
                .map_err(|_| AiProviderError::ContractMismatch)?;
            value.set_sensitive(true);
            headers.insert(AUTHORIZATION, value);
        }
        AiAuthConfig::Header { name, secret } => {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| AiProviderError::ContractMismatch)?;
            let secret =
                resolve_admin_auth(secret).map_err(|_| AiProviderError::ContractMismatch)?;
            let mut value = HeaderValue::from_str(secret.expose())
                .map_err(|_| AiProviderError::ContractMismatch)?;
            value.set_sensitive(true);
            headers.insert(name, value);
        }
    }
    Ok(headers)
}
