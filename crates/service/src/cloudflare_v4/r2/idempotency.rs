//! Generation-scoped idempotency for official R2 PUT-by-name.

use crate::cloudflare_v4::V4Error;
use crate::r2_http::R2ApiState;
use open_compute_core::{AccountId, BindingKind, ResourceState};
use open_compute_storage::ResourceRepository;

pub(super) fn create_fingerprint(
    api: &R2ApiState,
    account_id: AccountId,
    name: &str,
) -> Result<[u8; 32], V4Error> {
    let input = serde_json::to_vec(&serde_json::json!({
        "account": account_id,
        "name": name,
        "maxObjectBytes": api.config().max_object_bytes,
    }))
    .map_err(|_| V4Error::Internal)?;
    Ok(api.storage().crypto().fingerprint_request(&input))
}

pub(super) fn put_idempotency_key(
    api: &R2ApiState,
    account_id: AccountId,
    name: &str,
) -> Result<String, V4Error> {
    let resources = ResourceRepository::new(api.storage().db())
        .list(account_id, Some(BindingKind::R2Bucket))
        .map_err(|error| V4Error::from(&error))?;
    let mut latest: Option<(i64, String)> = None;
    for resource in resources
        .into_iter()
        .filter(|resource| resource.name == name && resource.state == ResourceState::Tombstoned)
    {
        let deleted_at = resource.deleted_at_ms.ok_or(V4Error::Internal)?;
        let generation = resource.id.to_string();
        if latest
            .as_ref()
            .is_none_or(|current| (deleted_at, &generation) > (current.0, &current.1))
        {
            latest = Some((deleted_at, generation));
        }
    }
    let generation = latest.map_or_else(|| "initial".to_owned(), |(_, value)| value);
    Ok(format!("cfv4:r2-put:{name}:{generation}"))
}
