//! D1 session bookmark admission and issuance on the private data plane.

use crate::d1_protocol::D1SessionConstraint;
use open_compute_core::{AccountId, ErrorCode, PlatformError, ResourceId};
use open_compute_storage::{D1Engine, SecretCrypto};

/// Stable sanitized error for invalid D1 session bookmarks.
pub(crate) fn session_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::D1SessionError,
        "D1 session bookmark is invalid for this database",
    )
}

/// Reject malformed, forged, future, or other-database bookmarks before execution.
pub(crate) fn apply_session(
    crypto: &SecretCrypto,
    account_id: AccountId,
    resource_id: ResourceId,
    engine: &D1Engine,
    session: &D1SessionConstraint,
) -> Result<(), PlatformError> {
    let D1SessionConstraint::Bookmark(token) = session else {
        return Ok(());
    };
    let version = crypto.open_d1_bookmark(account_id, resource_id, token)?;
    if version > engine.session_version()? {
        return Err(session_error());
    }
    Ok(())
}

/// Seal a fresh opaque bookmark after a successful session query.
pub(crate) fn issue_bookmark(
    crypto: &SecretCrypto,
    account_id: AccountId,
    resource_id: ResourceId,
    engine: &D1Engine,
    session: &D1SessionConstraint,
) -> Result<(Option<String>, u64), PlatformError> {
    let version = engine.session_version()?;
    match session {
        D1SessionConstraint::AlwaysPrimary => Ok((None, version)),
        D1SessionConstraint::FirstUnconstrained
        | D1SessionConstraint::FirstPrimary
        | D1SessionConstraint::Bookmark(_) => Ok((
            Some(crypto.seal_d1_bookmark(account_id, resource_id, version)?),
            version,
        )),
    }
}
