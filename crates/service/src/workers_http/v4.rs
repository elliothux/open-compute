//! Cloudflare v4 Worker protocol domain types and upload adapters.

#[path = "v4/account_subdomain.rs"]
mod account_subdomain;
#[path = "v4/asset_wire.rs"]
mod asset_wire;
#[path = "v4/assets.rs"]
pub(crate) mod assets;
#[path = "v4/authority.rs"]
mod authority;
#[path = "v4/cloning.rs"]
mod cloning;
#[path = "v4/do_lifecycle.rs"]
mod do_lifecycle;
#[path = "v4/domain.rs"]
mod domain;
#[path = "v4/download.rs"]
mod download;
#[path = "v4/errors.rs"]
mod errors;
#[path = "v4/handlers.rs"]
mod handlers;
#[path = "v4/json.rs"]
mod json;
#[path = "v4/model.rs"]
pub(crate) mod model;
#[path = "v4/multipart.rs"]
pub(crate) mod multipart;
#[path = "v4/mutations.rs"]
mod mutations;
#[path = "v4/projection.rs"]
mod projection;
#[path = "v4/query.rs"]
mod query;

pub(crate) use handlers::router;
