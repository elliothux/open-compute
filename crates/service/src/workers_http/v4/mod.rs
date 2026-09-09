//! Cloudflare v4 Worker protocol domain types and upload adapters.

mod account_subdomain;
mod asset_wire;
pub(crate) mod assets;
mod authority;
mod cloning;
mod do_lifecycle;
mod domain;
mod download;
mod errors;
mod handlers;
mod json;
pub(crate) mod model;
pub(crate) mod multipart;
mod mutations;
mod observability;
mod projection;
mod query;
mod sdk_multipart;

pub(crate) use handlers::router;
pub(crate) use observability::signed_router;
