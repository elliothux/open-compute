//! Durable Object namespace authority and object-generation registry.
//!
//! Namespace and object transactions remain together because they share one SQLite authority
//! boundary and must be audited as a single generation-fencing protocol.

use crate::catalog_page::{CatalogColumns, build_catalog_sql, record_catalog_cursor};
use crate::{
    BindingRepository, CatalogCursor, CatalogDirection, CatalogListPage, CatalogSort,
    PlatformStorage, ResourceRecord, ResourceRepository, normalize_catalog_limit,
    search_as_resource_id,
};
use open_compute_core::{
    AccountId, BindingId, BindingKind, DurableObjectId, DurableObjectState, ErrorCode,
    PlatformError, ResourceId, ResourceState, VersionId, WorkerId, durable_object_namespace_prefix,
};
use rusqlite::{OptionalExtension, params, params_from_iter};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::str::FromStr;

#[path = "durable_object_migrations.rs"]
mod worker_migrations;
pub(crate) use worker_migrations::publish_worker_migration_tx;
pub use worker_migrations::{
    DurableObjectClassRename, DurableObjectMigrationHead, DurableObjectMigrationPlan,
    DurableObjectMigrationPreparation,
};

mod helpers;
mod model;
mod namespaces;
mod objects;

use helpers::*;
pub use model::*;
use model::{AlarmDispatchAuthorityRow, DispatchAuthorityRow};

#[cfg(test)]
#[path = "durable_object_migrations_tests.rs"]
mod migration_tests;
#[cfg(test)]
#[path = "durable_objects_tests.rs"]
mod tests;
