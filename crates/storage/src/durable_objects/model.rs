use super::*;

/// Product schema version for P0.7 namespace rows.
pub const DO_NAMESPACE_SCHEMA_VERSION: u32 = 1;

/// Immutable Durable Object namespace product row with its resource authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DurableObjectNamespaceRecord {
    /// Generic resource lifecycle row.
    pub resource: ResourceRecord,
    /// Worker that owns the exported class and stable storage identity.
    pub owner_worker_id: WorkerId,
    /// Immutable named export used for dynamic facet construction.
    pub class_name: String,
    /// Stable Worker storage identity copied and checked at creation.
    pub do_storage_id: String,
    /// Opaque stable namespace storage identity.
    pub namespace_storage_key: String,
    /// Product schema version.
    pub schema_version: u32,
    /// Creation timestamp copied from the resource row.
    pub created_at_ms: i64,
}

/// One lifecycle generation in the object registry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DurableObjectRecord {
    /// Owning namespace resource.
    pub namespace_resource_id: ResourceId,
    /// Canonical public object identity.
    pub object_id: DurableObjectId,
    /// Monotonic generation for delete/recreate fencing.
    pub generation: u64,
    /// Current lifecycle state.
    pub state: DurableObjectState,
    /// Generation creation time.
    pub created_at_ms: i64,
    /// Last transition time.
    pub updated_at_ms: i64,
    /// Tombstone time.
    pub deleted_at_ms: Option<i64>,
}

/// One bounded page of object registry rows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableObjectListPage {
    /// Rows selected for this page in deterministic order.
    pub objects: Vec<DurableObjectRecord>,
    /// Opaque cursor for the next page when more rows remain.
    pub next_cursor: Option<String>,
}

/// Encode one list cursor from the last row returned on a page.
#[must_use]
pub fn encode_object_list_cursor(record: &DurableObjectRecord) -> String {
    format!("{}:{}", record.object_id, record.generation)
}

/// Decode an opaque object-list cursor into its SQL sort key.
pub fn decode_object_list_cursor(cursor: &str) -> Result<(DurableObjectId, u64), PlatformError> {
    let (object, generation) = cursor.rsplit_once(':').ok_or_else(invalid_list_cursor)?;
    if object.is_empty() || generation.is_empty() {
        return Err(invalid_list_cursor());
    }
    let object_id = DurableObjectId::from_str(object).map_err(|_| invalid_list_cursor())?;
    let generation = generation
        .parse::<u64>()
        .map_err(|_| invalid_list_cursor())?;
    if generation == 0 {
        return Err(invalid_list_cursor());
    }
    Ok((object_id, generation))
}

/// Trusted metadata returned only to the private system-Worker router.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizedDurableObjectDispatch {
    /// Account derived through binding authority.
    pub account_id: AccountId,
    /// Namespace resource resolved from the immutable binding.
    pub namespace_resource_id: ResourceId,
    /// Namespace owner Worker.
    pub worker_id: WorkerId,
    /// Current active version.
    pub version_id: VersionId,
    /// Current immutable descriptor digest.
    pub worker_code_sha256: String,
    /// Monotonic route/execution generation.
    pub route_generation: u64,
    /// Exported Durable Object class.
    pub class_name: String,
    /// Public object identity.
    pub object_id: DurableObjectId,
    /// Live object registry generation.
    pub object_generation: u64,
    /// Opaque name passed to native `idFromName()`.
    pub host_key: String,
}

/// Minimal native-delete capability derived after the object generation is fenced.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizedDurableObjectDelete {
    /// Canonical public object identity used only for generation cross-checking.
    pub object_id: DurableObjectId,
    /// Fenced lifecycle generation that owns the physical host key.
    pub object_generation: u64,
    /// Opaque keyed actor identity; it cannot be selected by tenant input.
    pub host_key: String,
}

pub(super) struct DispatchAuthorityRow {
    pub(super) worker_id: String,
    pub(super) active_version_id: String,
    pub(super) route_generation: i64,
    pub(super) worker_storage_id: String,
    pub(super) worker_code_sha256: Vec<u8>,
    pub(super) class_name: String,
    pub(super) namespace_storage_id: String,
    pub(super) namespace_storage_key: String,
}

pub(super) struct AlarmDispatchAuthorityRow {
    pub(super) account_id: String,
    pub(super) worker_id: String,
    pub(super) version_id: String,
    pub(super) route_generation: i64,
    pub(super) worker_code_sha256: Vec<u8>,
    pub(super) class_name: String,
    pub(super) namespace_storage_id: String,
    pub(super) namespace_storage_key: String,
}

/// Durable Object repositories over central platform storage and key authority.
#[derive(Clone, Copy, Debug)]
pub struct DurableObjectRepository<'a> {
    pub(super) storage: &'a PlatformStorage,
}
