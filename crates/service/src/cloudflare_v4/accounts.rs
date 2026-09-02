//! Minimal fixed-installation Cloudflare account discovery surface.

use super::{
    V4Error, V4Permission, V4RequestContext, V4ResultInfo, V4Role, error_response,
    paginated_response, request_context, success_response,
};
use crate::http::HttpState;
use axum::Router;
use axum::extract::{Path, Request, State};
use axum::response::Response;
use axum::routing::get;
use open_compute_core::{AccountId, PlatformId, ResourceId};
use serde::Serialize;
use sha2::{Digest, Sha256};
use url::form_urlencoded;

const ACCOUNT_NAME: &str = "default";
const USER_EMAIL: &str = "operator@open-compute.invalid";

/// Stable mapping from the installation identity to the public v4 account.
#[derive(Clone, Debug)]
pub(crate) struct AccountAuthority {
    internal_id: AccountId,
    public_id: String,
    user_id: String,
    membership_id: String,
    platform_id: PlatformId,
    created_at_ms: i64,
}

/// Domain separators for stable public resource identifiers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum V4ResourceKind {
    /// KV namespace resource.
    KvNamespace,
    /// D1 database resource.
    D1Database,
    /// Durable Object namespace resource.
    DurableObjectNamespace,
}

impl V4ResourceKind {
    const fn scope(self) -> &'static str {
        match self {
            Self::KvNamespace => "kv-namespace",
            Self::D1Database => "d1-database",
            Self::DurableObjectNamespace => "durable-object-namespace",
        }
    }
}

impl AccountAuthority {
    /// Build the one-account Day 1 mapping without exposing internal identifiers.
    pub(crate) fn new(platform_id: PlatformId, internal_id: AccountId, created_at_ms: i64) -> Self {
        Self {
            internal_id,
            public_id: stable_id("account", platform_id, None),
            user_id: stable_id("user", platform_id, None),
            membership_id: stable_id("membership", platform_id, None),
            platform_id,
            created_at_ms,
        }
    }

    /// Resolve the public account identifier to the internal storage scope.
    pub(crate) fn resolve(&self, public_id: &str) -> Result<AccountId, V4Error> {
        (self.public_id == public_id)
            .then_some(self.internal_id)
            .ok_or(V4Error::NotFound)
    }

    /// Public Cloudflare-compatible account identifier.
    pub(crate) fn public_id(&self) -> &str {
        &self.public_id
    }

    /// Map an internal resource identity to a stable, domain-separated public 32-hex ID.
    pub(crate) fn public_resource_id(&self, kind: V4ResourceKind, id: ResourceId) -> String {
        stable_id(kind.scope(), self.platform_id, Some(&id.to_string()))
    }

    /// Compare a public resource ID without exposing the internal UUID.
    pub(crate) fn matches_public_resource_id(
        &self,
        kind: V4ResourceKind,
        id: ResourceId,
        public: &str,
    ) -> bool {
        public.len() == 32 && self.public_resource_id(kind, id) == public
    }

    fn account(&self) -> Result<Account, V4Error> {
        Ok(Account {
            id: self.public_id.clone(),
            name: ACCOUNT_NAME,
            kind: "standard",
            created_on: timestamp(self.created_at_ms)?,
        })
    }

    fn token_id(&self, role: V4Role) -> String {
        let role = match role {
            V4Role::Admin => "admin",
            V4Role::Deployer => "deployer",
            V4Role::ReadOnly => "read-only",
        };
        stable_id(role, self.platform_id, Some("token"))
    }
}

pub(super) fn router() -> Router<HttpState> {
    Router::new()
        .route("/user", get(user))
        .route("/user/tokens/verify", get(verify_token))
        .route("/accounts", get(list_accounts))
        .route("/accounts/{account_id}", get(get_account))
        .route("/memberships", get(list_memberships))
}

async fn user(State(state): State<HttpState>, request: Request) -> Response {
    let context = match read_context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(account) = state.cloudflare_v4_account() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    success_response(
        context,
        User {
            id: account.user_id.clone(),
            email: USER_EMAIL,
        },
    )
}

async fn verify_token(State(state): State<HttpState>, request: Request) -> Response {
    let context = match read_context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(account) = state.cloudflare_v4_account() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    success_response(
        context,
        TokenVerification {
            id: account.token_id(context.role()),
            status: "active",
        },
    )
}

async fn list_accounts(State(state): State<HttpState>, request: Request) -> Response {
    let context = match read_context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let query = match CollectionQuery::parse(request.uri().query(), CollectionKind::Accounts) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(authority) = state.cloudflare_v4_account() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let account = match authority.account() {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let matches = query
        .name
        .as_deref()
        .is_none_or(|name| name == account.name);
    let visible = usize::from(matches && query.page == 1);
    success_collection(
        context,
        matches
            .then_some(account)
            .filter(|_| query.page == 1)
            .into_iter()
            .collect(),
        query,
        visible,
        usize::from(matches),
    )
}

async fn get_account(
    State(state): State<HttpState>,
    Path(account_id): Path<String>,
    request: Request,
) -> Response {
    let context = match read_context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(authority) = state.cloudflare_v4_account() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    if let Err(error) = authority.resolve(&account_id) {
        return error_response(error, context.request_id());
    }
    match authority.account() {
        Ok(account) => success_response(context, account),
        Err(error) => error_response(error, context.request_id()),
    }
}

async fn list_memberships(State(state): State<HttpState>, request: Request) -> Response {
    let context = match read_context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let query = match CollectionQuery::parse(request.uri().query(), CollectionKind::Memberships) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(authority) = state.cloudflare_v4_account() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let account = match authority.account() {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let matches = query
        .name
        .as_deref()
        .is_none_or(|name| name == account.name);
    let membership = Membership {
        id: authority.membership_id.clone(),
        account,
        api_access_enabled: true,
        roles: vec![role_name(context.role())],
        status: "accepted",
    };
    let visible = usize::from(matches && query.page == 1);
    success_collection(
        context,
        matches
            .then_some(membership)
            .filter(|_| query.page == 1)
            .into_iter()
            .collect(),
        query,
        visible,
        usize::from(matches),
    )
}

fn read_context(request: &Request, permission: V4Permission) -> Result<V4RequestContext, Response> {
    let context = request_context(request)?;
    context
        .require(permission)
        .map_err(|error| error_response(error, context.request_id()))?;
    Ok(context)
}

fn success_collection<T: Serialize>(
    context: V4RequestContext,
    result: Vec<T>,
    query: CollectionQuery,
    count: usize,
    total_count: usize,
) -> Response {
    paginated_response(
        context,
        result,
        V4ResultInfo {
            page: query.page,
            per_page: query.per_page,
            count,
            total_count,
        },
    )
}

#[derive(Clone, Copy)]
enum CollectionKind {
    Accounts,
    Memberships,
}

struct CollectionQuery {
    page: usize,
    per_page: usize,
    name: Option<String>,
}

impl CollectionQuery {
    fn parse(raw: Option<&str>, kind: CollectionKind) -> Result<Self, V4Error> {
        let mut page = 1;
        let mut per_page = 20;
        let mut name = None;
        for (key, value) in form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
            match key.as_ref() {
                "page" => page = parse_usize(&value)?,
                "per_page" => {
                    per_page = parse_usize(&value)?;
                    if !(5..=50).contains(&per_page) {
                        return Err(V4Error::InvalidField("/per_page"));
                    }
                }
                "name" if matches!(kind, CollectionKind::Accounts) => name = one(name, &value)?,
                "account.name" | "name" if matches!(kind, CollectionKind::Memberships) => {
                    name = one(name, &value)?;
                }
                "direction" if value == "asc" || value == "desc" => {}
                "order"
                    if matches!(kind, CollectionKind::Memberships)
                        && matches!(value.as_ref(), "id" | "account.name" | "status") => {}
                "status" if matches!(kind, CollectionKind::Memberships) && value == "accepted" => {}
                _ => {
                    return Err(V4Error::InvalidRequest);
                }
            }
        }
        Ok(Self {
            page,
            per_page,
            name,
        })
    }
}

fn one(existing: Option<String>, value: &str) -> Result<Option<String>, V4Error> {
    if existing.is_some() || value.is_empty() || value.len() > 100 {
        return Err(V4Error::InvalidRequest);
    }
    Ok(Some(value.to_owned()))
}

fn parse_usize(value: &str) -> Result<usize, V4Error> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| V4Error::InvalidRequest)?;
    (parsed > 0)
        .then_some(parsed)
        .ok_or(V4Error::InvalidRequest)
}

fn role_name(role: V4Role) -> &'static str {
    match role {
        V4Role::Admin => "Open Compute Administrator",
        V4Role::Deployer => "Open Compute Deployer",
        V4Role::ReadOnly => "Open Compute Read Only",
    }
}

fn timestamp(value: i64) -> Result<String, V4Error> {
    jiff::Timestamp::from_millisecond(value)
        .map(|timestamp| timestamp.to_string())
        .map_err(|_| V4Error::Internal)
}

fn stable_id(scope: &str, platform: PlatformId, suffix: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"open-compute/cloudflare-v4/v1\0");
    hasher.update(scope.as_bytes());
    hasher.update([0]);
    hasher.update(platform.as_uuid().as_bytes());
    if let Some(suffix) = suffix {
        hasher.update([0]);
        hasher.update(suffix.as_bytes());
    }
    hex::encode(hasher.finalize())[..32].to_owned()
}

#[derive(Serialize)]
struct User {
    id: String,
    email: &'static str,
}

#[derive(Serialize)]
struct TokenVerification {
    id: String,
    status: &'static str,
}

#[derive(Clone, Serialize)]
struct Account {
    id: String,
    name: &'static str,
    #[serde(rename = "type")]
    kind: &'static str,
    created_on: String,
}

#[derive(Serialize)]
struct Membership {
    id: String,
    account: Account,
    api_access_enabled: bool,
    roles: Vec<&'static str>,
    status: &'static str,
}
