use crate::cloudflare_v4::V4Error;
use open_compute_core::ResourceState;
use open_compute_storage::{
    CatalogCursor, CatalogDirection, CatalogSort, decode_catalog_cursor, decode_object_list_cursor,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::str::FromStr;

pub(super) struct NamespaceListQuery {
    pub(super) search: Option<String>,
    pub(super) status: Option<ResourceState>,
    pub(super) sort: CatalogSort,
    pub(super) direction: CatalogDirection,
    pub(super) cursor: Option<CatalogCursor>,
    pub(super) per_page: u16,
}

impl NamespaceListQuery {
    pub(super) fn parse(query: Option<&str>) -> Result<Self, V4Error> {
        let values = unique_query(query)?;
        if values.keys().any(|key| {
            !matches!(
                key.as_str(),
                "search" | "status" | "sort" | "direction" | "cursor" | "per_page"
            )
        }) {
            return Err(V4Error::InvalidRequest);
        }
        let sort = parse_catalog(values.get("sort"), CatalogSort::Name)?;
        let direction = parse_catalog(values.get("direction"), CatalogDirection::Asc)?;
        let cursor = values
            .get("cursor")
            .map(|value| decode_catalog_cursor(value).map_err(|error| V4Error::from(&error)))
            .transpose()?;
        if cursor
            .as_ref()
            .is_some_and(|value| value.sort != sort || value.direction != direction)
        {
            return Err(V4Error::InvalidRequest);
        }
        Ok(Self {
            search: values
                .get("search")
                .cloned()
                .filter(|value| !value.is_empty()),
            status: values
                .get("status")
                .map(|value| ResourceState::from_str(value).map_err(|_| V4Error::InvalidRequest))
                .transpose()?,
            sort,
            direction,
            cursor,
            per_page: parse_per_page(values.get("per_page"))?,
        })
    }
}

pub(super) struct ObjectListQuery {
    pub(super) cursor: Option<(open_compute_core::DurableObjectId, u64)>,
    pub(super) per_page: u16,
}

impl ObjectListQuery {
    pub(super) fn parse(query: Option<&str>) -> Result<Self, V4Error> {
        let values = unique_query(query)?;
        if values
            .keys()
            .any(|key| !matches!(key.as_str(), "cursor" | "per_page"))
        {
            return Err(V4Error::InvalidRequest);
        }
        Ok(Self {
            cursor: values
                .get("cursor")
                .map(|value| {
                    decode_object_list_cursor(value).map_err(|error| V4Error::from(&error))
                })
                .transpose()?,
            per_page: parse_per_page(values.get("per_page"))?,
        })
    }
}

fn unique_query(query: Option<&str>) -> Result<BTreeMap<String, String>, V4Error> {
    let mut values = BTreeMap::new();
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        if values
            .insert(key.into_owned(), value.into_owned())
            .is_some()
        {
            return Err(V4Error::InvalidRequest);
        }
    }
    Ok(values)
}

fn parse_catalog<T: FromStr>(value: Option<&String>, default: T) -> Result<T, V4Error> {
    value.map_or(Ok(default), |value| {
        value.parse().map_err(|_| V4Error::InvalidRequest)
    })
}

fn parse_per_page(value: Option<&String>) -> Result<u16, V4Error> {
    let value = value
        .map_or(Ok(100), |value| value.parse::<u16>())
        .map_err(|_| V4Error::InvalidRequest)?;
    (value > 0 && value <= 1_000)
        .then_some(value)
        .ok_or(V4Error::InvalidRequest)
}

#[derive(Serialize)]
pub(super) struct DurableObjectNamespace {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) script_name: String,
    pub(super) class_name: String,
    pub(super) state: &'static str,
    pub(super) availability: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) availability_code: Option<String>,
    pub(super) spec_generation: u64,
    pub(super) schema_version: u32,
    pub(super) created_on: String,
    pub(super) modified_on: String,
}

#[derive(Serialize)]
pub(super) struct DurableObjectRecord {
    pub(super) id: String,
    pub(super) namespace_id: String,
    pub(super) generation: u64,
    pub(super) state: &'static str,
    pub(super) created_on: String,
    pub(super) modified_on: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) deleted_on: Option<String>,
}

#[derive(Serialize)]
pub(super) struct DurableObjectNamespacePage {
    pub(super) items: Vec<DurableObjectNamespace>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) next_cursor: Option<String>,
}

#[derive(Serialize)]
pub(super) struct DurableObjectRecordPage {
    pub(super) items: Vec<DurableObjectRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) next_cursor: Option<String>,
}
