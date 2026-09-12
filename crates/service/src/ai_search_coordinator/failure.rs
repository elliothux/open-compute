//! Stable indexing failure classification and bounded retry policy.

use super::limit;
use crate::ai_provider::AiProviderError;
use crate::metrics::AiProviderOutcome;
use open_compute_core::{ErrorCode, PlatformError};

pub(super) const MAX_TRANSIENT_ATTEMPTS: u32 = 5;
pub(super) const INDEX_FAILURE_CODE: &str = "AI_SEARCH_INDEX_FAILED";
pub(super) const PROVIDER_UNAVAILABLE_CODE: &str = "AI_PROVIDER_UNAVAILABLE";
pub(super) const PROVIDER_RATE_LIMITED_CODE: &str = "AI_PROVIDER_RATE_LIMITED";
pub(super) const PROVIDER_TIMEOUT_CODE: &str = "AI_PROVIDER_TIMEOUT";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Failure {
    Transient {
        retry_after_seconds: Option<u64>,
        message_code: &'static str,
    },
    Permanent(&'static str),
    EmbeddingInputTooLarge,
}

pub(super) fn retry_delay(base_ms: u64, attempt: u32) -> Result<u64, PlatformError> {
    if attempt == 0 || attempt >= MAX_TRANSIENT_ATTEMPTS {
        return Err(limit());
    }
    let exponent = attempt - 1;
    base_ms.checked_mul(1_u64 << exponent).ok_or_else(limit)
}

pub(super) fn classify_provider(error: AiProviderError) -> Failure {
    match error {
        AiProviderError::RateLimited {
            retry_after_seconds,
        } => Failure::Transient {
            retry_after_seconds,
            message_code: PROVIDER_RATE_LIMITED_CODE,
        },
        AiProviderError::Transient => Failure::Transient {
            retry_after_seconds: None,
            message_code: PROVIDER_UNAVAILABLE_CODE,
        },
        AiProviderError::Timeout => Failure::Transient {
            retry_after_seconds: None,
            message_code: PROVIDER_TIMEOUT_CODE,
        },
        _ => Failure::Permanent(INDEX_FAILURE_CODE),
    }
}

pub(super) const fn provider_outcome(error: AiProviderError) -> AiProviderOutcome {
    match error {
        AiProviderError::InvalidRequest | AiProviderError::ContractMismatch => {
            AiProviderOutcome::Invalid
        }
        AiProviderError::Unauthorized => AiProviderOutcome::Unauthorized,
        AiProviderError::RateLimited { .. } => AiProviderOutcome::RateLimited,
        AiProviderError::Transient => AiProviderOutcome::Transient,
        AiProviderError::Permanent => AiProviderOutcome::Permanent,
        AiProviderError::Timeout => AiProviderOutcome::Timeout,
        AiProviderError::MalformedResponse => AiProviderOutcome::Malformed,
    }
}

pub(super) fn classify_platform(error: &PlatformError) -> Failure {
    match error.code() {
        ErrorCode::ObjectStorageUnavailable
        | ErrorCode::PlatformUnavailable
        | ErrorCode::DocumentUnavailable
        | ErrorCode::DocumentTimeout
        | ErrorCode::R2Overloaded
        | ErrorCode::R2ProviderUnavailable => Failure::Transient {
            retry_after_seconds: None,
            message_code: error.code().as_str(),
        },
        _ => Failure::Permanent(error.code().as_str()),
    }
}
