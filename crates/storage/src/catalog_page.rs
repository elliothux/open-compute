//! Shared bounded catalog list pagination helpers.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use open_compute_core::{ErrorCode, PlatformError, QueueId, ResourceId, WorkerId, WorkflowId};
use rusqlite::types::Value;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Supported server-side catalog sort fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CatalogSort {
    /// Display name.
    Name,
    /// Creation timestamp.
    CreatedAt,
    /// Last update timestamp.
    UpdatedAt,
}

impl CatalogSort {
    /// Stable Operator API query token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::CreatedAt => "createdAt",
            Self::UpdatedAt => "updatedAt",
        }
    }
}

impl FromStr for CatalogSort {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "name" => Ok(Self::Name),
            "createdAt" => Ok(Self::CreatedAt),
            "updatedAt" => Ok(Self::UpdatedAt),
            _ => Err(invalid_catalog_query()),
        }
    }
}

/// Supported server-side catalog sort direction.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CatalogDirection {
    /// Ascending values and identities.
    Asc,
    /// Descending values and identities.
    Desc,
}

impl CatalogDirection {
    /// SQL comparison used after a cursor.
    #[must_use]
    pub const fn comparison(self) -> &'static str {
        match self {
            Self::Asc => ">",
            Self::Desc => "<",
        }
    }

    /// SQL ordering token.
    #[must_use]
    pub const fn sql(self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
        }
    }
}

impl FromStr for CatalogDirection {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "asc" => Ok(Self::Asc),
            "desc" => Ok(Self::Desc),
            _ => Err(invalid_catalog_query()),
        }
    }
}

/// Cursor value for one selected catalog sort field.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum CatalogCursorValue {
    /// String sort value.
    Text(String),
    /// Signed millisecond timestamp sort value.
    Integer(i64),
}

/// Opaque cursor payload bound to the selected sort configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogCursor {
    /// Sort field used to create the page.
    pub sort: CatalogSort,
    /// Sort direction used to create the page.
    pub direction: CatalogDirection,
    /// Last row's selected sort value.
    pub value: CatalogCursorValue,
    /// Last row's stable identity.
    pub id: String,
}

/// Encode a sort-bound catalog cursor as base64url JSON.
#[must_use]
pub fn encode_catalog_cursor(cursor: &CatalogCursor) -> String {
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(cursor).unwrap_or_default())
}

/// Decode and validate one sort-bound catalog cursor.
pub fn decode_catalog_cursor(cursor: &str) -> Result<CatalogCursor, PlatformError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| invalid_catalog_cursor())?;
    let cursor: CatalogCursor =
        serde_json::from_slice(&bytes).map_err(|_| invalid_catalog_cursor())?;
    if cursor.id.is_empty() {
        return Err(invalid_catalog_cursor());
    }
    Ok(cursor)
}

/// Stable invalid-query error for catalog filters and sort controls.
pub fn invalid_catalog_query() -> PlatformError {
    PlatformError::new(ErrorCode::ConfigInvalid, "catalog list query is invalid")
}

/// Whitelisted SQL column expressions for one catalog query.
#[derive(Clone, Copy)]
pub(crate) struct CatalogColumns<'a> {
    pub(crate) id: &'a str,
    pub(crate) name: &'a str,
    pub(crate) state: &'a str,
    pub(crate) created_at: &'a str,
    pub(crate) updated_at: &'a str,
}

/// Prepared SQL text and positional values for a bounded catalog page.
pub(crate) struct CatalogSql {
    pub(crate) text: String,
    pub(crate) values: Vec<Value>,
}

/// Build a bounded catalog query from compile-time-owned SQL expressions.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_catalog_sql(
    base: &str,
    columns: CatalogColumns<'_>,
    account_id: String,
    search_needle: Option<String>,
    exact_id: Option<String>,
    status: Option<String>,
    sort: CatalogSort,
    direction: CatalogDirection,
    after: Option<CatalogCursor>,
    fetch: u32,
) -> Result<CatalogSql, PlatformError> {
    let sort_expression = match sort {
        CatalogSort::Name => columns.name,
        CatalogSort::CreatedAt => columns.created_at,
        CatalogSort::UpdatedAt => columns.updated_at,
    };
    let mut text = base.to_string();
    let mut values = vec![Value::Text(account_id)];
    if let Some(id) = exact_id {
        text.push_str(&format!(" AND {} = ?", columns.id));
        values.push(Value::Text(id));
    } else if let Some(needle) = search_needle {
        text.push_str(&format!(" AND INSTR(LOWER({}), ?) > 0", columns.name));
        values.push(Value::Text(needle));
    }
    if let Some(status) = status {
        text.push_str(&format!(" AND {} = ?", columns.state));
        values.push(Value::Text(status));
    }
    if let Some(cursor) = after {
        if cursor.sort != sort || cursor.direction != direction {
            return Err(invalid_catalog_cursor());
        }
        let cursor_value = match (sort, cursor.value) {
            (CatalogSort::Name, CatalogCursorValue::Text(value)) => Value::Text(value),
            (
                CatalogSort::CreatedAt | CatalogSort::UpdatedAt,
                CatalogCursorValue::Integer(value),
            ) => Value::Integer(value),
            _ => return Err(invalid_catalog_cursor()),
        };
        let comparison = direction.comparison();
        text.push_str(&format!(
            " AND ({sort_expression} {comparison} ? OR ({sort_expression} = ? AND {} {comparison} ?))",
            columns.id,
        ));
        values.push(cursor_value.clone());
        values.push(cursor_value);
        values.push(Value::Text(cursor.id));
    }
    text.push_str(&format!(
        " ORDER BY {sort_expression} {}, {} {} LIMIT ?",
        direction.sql(),
        columns.id,
        direction.sql(),
    ));
    values.push(Value::Integer(i64::from(fetch)));
    Ok(CatalogSql { text, values })
}

/// Encode the next cursor for a catalog record.
pub(crate) fn record_catalog_cursor(
    sort: CatalogSort,
    direction: CatalogDirection,
    name: &str,
    created_at_ms: i64,
    updated_at_ms: i64,
    id: &str,
) -> String {
    let value = match sort {
        CatalogSort::Name => CatalogCursorValue::Text(name.to_string()),
        CatalogSort::CreatedAt => CatalogCursorValue::Integer(created_at_ms),
        CatalogSort::UpdatedAt => CatalogCursorValue::Integer(updated_at_ms),
    };
    encode_catalog_cursor(&CatalogCursor {
        sort,
        direction,
        value,
        id: id.to_string(),
    })
}

/// Default page size for catalog list operations.
pub const DEFAULT_CATALOG_LIST_LIMIT: u16 = 100;

/// Maximum page size for catalog list operations.
pub const MAX_CATALOG_LIST_LIMIT: u16 = 1_000;

/// One bounded page of catalog rows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogListPage<T> {
    /// Rows selected for this page in deterministic order.
    pub items: Vec<T>,
    /// Opaque cursor for the next page when more rows remain.
    pub next_cursor: Option<String>,
}

/// Sort key for catalogs ordered by display name and resource id.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NameIdCursor {
    /// Resource display name.
    pub name: String,
    /// Resource identity.
    pub id: ResourceId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct NameIdCursorPayload {
    name: String,
    id: String,
}

/// Sort key for catalogs ordered by creation time and string id.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatedIdCursor {
    /// Creation timestamp in milliseconds.
    pub created_at_ms: i64,
    /// Stable string identity.
    pub id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct CreatedIdCursorPayload {
    created_at_ms: i64,
    id: String,
}

/// Clamp one caller-supplied catalog limit to the supported range.
#[must_use]
pub fn normalize_catalog_limit(limit: u16) -> u16 {
    limit.clamp(1, MAX_CATALOG_LIST_LIMIT)
}

/// Encode one `(name, id)` catalog cursor as opaque base64url JSON.
#[must_use]
pub fn encode_name_id_cursor(name: &str, id: ResourceId) -> String {
    let payload = NameIdCursorPayload {
        name: name.to_string(),
        id: id.to_string(),
    };
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap_or_default())
}

/// Decode one opaque `(name, id)` catalog cursor.
pub fn decode_name_id_cursor(cursor: &str) -> Result<NameIdCursor, PlatformError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| invalid_catalog_cursor())?;
    let payload: NameIdCursorPayload =
        serde_json::from_slice(&bytes).map_err(|_| invalid_catalog_cursor())?;
    if payload.name.is_empty() {
        return Err(invalid_catalog_cursor());
    }
    let id = ResourceId::from_str(&payload.id).map_err(|_| invalid_catalog_cursor())?;
    Ok(NameIdCursor {
        name: payload.name,
        id,
    })
}

/// Encode one `(created_at_ms, id)` catalog cursor as opaque base64url JSON.
#[must_use]
pub fn encode_created_id_cursor(created_at_ms: i64, id: &str) -> String {
    let payload = CreatedIdCursorPayload {
        created_at_ms,
        id: id.to_string(),
    };
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap_or_default())
}

/// Decode one opaque `(created_at_ms, id)` catalog cursor.
pub fn decode_created_id_cursor(cursor: &str) -> Result<CreatedIdCursor, PlatformError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| invalid_catalog_cursor())?;
    let payload: CreatedIdCursorPayload =
        serde_json::from_slice(&bytes).map_err(|_| invalid_catalog_cursor())?;
    if payload.id.is_empty() {
        return Err(invalid_catalog_cursor());
    }
    Ok(CreatedIdCursor {
        created_at_ms: payload.created_at_ms,
        id: payload.id,
    })
}

/// Return whether one search string should be treated as an exact resource id lookup.
pub fn search_as_resource_id(search: &str) -> Option<ResourceId> {
    ResourceId::from_str(search.trim()).ok()
}

/// Return whether one search string should be treated as an exact Worker id lookup.
pub fn search_as_worker_id(search: &str) -> Option<WorkerId> {
    WorkerId::from_str(search.trim()).ok()
}

/// Return whether one search string should be treated as an exact Queue id lookup.
pub fn search_as_queue_id(search: &str) -> Option<QueueId> {
    QueueId::from_str(search.trim()).ok()
}

/// Return whether one search string should be treated as an exact Workflow id lookup.
pub fn search_as_workflow_id(search: &str) -> Option<WorkflowId> {
    WorkflowId::from_str(search.trim()).ok()
}

/// Stable invalid-cursor error for catalog pagination.
pub fn invalid_catalog_cursor() -> PlatformError {
    PlatformError::new(ErrorCode::ConfigInvalid, "catalog list cursor is invalid")
}
