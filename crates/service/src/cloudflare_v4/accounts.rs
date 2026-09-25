//! Minimal fixed-installation Cloudflare account discovery surface.

use super::{
    HttpError, V4Error, V4Permission, V4RequestContext, V4ResultInfo, V4Role, error_response,
    paginated_response, request_context, success_response,
};
use crate::http::HttpState;
use crate::run::daemon_control::InstanceView;
use axum::Router;
use axum::extract::{Path, Request, State};
use axum::response::Response;
use axum::routing::get;
use open_compute_core::{InstanceId, QueueConsumerId, QueueId, RequestId, ResourceId, WorkerId};
use serde::Serialize;
use sha2::{Digest, Sha256};
use url::form_urlencoded;

const ACCOUNT_NAME: &str = "default";
const USER_EMAIL: &str = "operator@open-compute.invalid";

/// Cloudflare wire projections for one instance.
#[derive(Clone, Debug)]
pub(crate) struct V4InstanceContext {
    instance_id: InstanceId,
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

impl V4InstanceContext {
    /// Build Cloudflare wire projections from the single durable instance identity.
    pub(crate) fn new(instance_id: InstanceId, created_at_ms: i64) -> Self {
        Self {
            instance_id,
            created_at_ms,
        }
    }

    /// Parse the Cloudflare account path as the instance identity.
    pub(crate) fn resolve(&self, public_id: &str) -> Result<InstanceId, V4Error> {
        (self.instance_id.as_str() == public_id)
            .then_some(self.instance_id)
            .ok_or(V4Error::NotFound)
    }

    /// Durable instance identity used by product data planes.
    pub(crate) const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    /// Public Cloudflare-compatible account identifier.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn public_id(&self) -> &str {
        self.instance_id.as_str()
    }

    /// Return the stable, intentionally non-DNS account label required by the pinned Wrangler
    /// Workflow deployment preflight.
    pub(crate) fn workers_dev_prerequisite_label(&self) -> String {
        format!("_open-compute-unroutable-{}", self.instance_id)
    }

    /// Map an internal resource identity to a stable, domain-separated public 32-hex ID.
    pub(crate) fn public_resource_id(&self, kind: V4ResourceKind, id: ResourceId) -> String {
        stable_id(kind.scope(), self.instance_id, Some(&id.to_string()))
    }

    /// Derive the stable non-secret service tag exposed by Cloudflare's legacy
    /// service metadata probe used by the pinned Wrangler deploy workflow.
    pub(crate) fn public_worker_tag(&self, id: WorkerId) -> String {
        stable_id("worker-tag", self.instance_id, Some(&id.to_string()))
    }

    /// Compare a public Worker identifier without exposing the internal UUID.
    pub(crate) fn matches_public_worker_tag(&self, id: WorkerId, public: &str) -> bool {
        public.len() == 32 && self.public_worker_tag(id) == public
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

    /// Map a Queue identity to its Cloudflare-shaped stable public ID.
    pub(crate) fn public_queue_id(&self, id: QueueId) -> String {
        stable_id("queue", self.instance_id, Some(&id.to_string()))
    }

    /// Compare a public Queue ID without exposing the internal UUID.
    pub(crate) fn matches_public_queue_id(&self, id: QueueId, public: &str) -> bool {
        public.len() == 32 && self.public_queue_id(id) == public
    }

    /// Map a Queue consumer identity to its Cloudflare-shaped stable public ID.
    pub(crate) fn public_queue_consumer_id(&self, id: QueueConsumerId) -> String {
        stable_id("queue-consumer", self.instance_id, Some(&id.to_string()))
    }

    /// Compare a public Queue consumer ID without exposing its internal UUID.
    pub(crate) fn matches_public_queue_consumer_id(
        &self,
        id: QueueConsumerId,
        public: &str,
    ) -> bool {
        public.len() == 32 && self.public_queue_consumer_id(id) == public
    }

    fn account(&self) -> Result<Account, V4Error> {
        Ok(Account {
            id: self.instance_id.to_string(),
            name: ACCOUNT_NAME.to_owned(),
            kind: "standard",
            created_on: Some(crate::cloudflare_v4::iso_timestamp(self.created_at_ms)?),
        })
    }

    fn token_id(&self, role: V4Role) -> String {
        let role = match role {
            V4Role::Admin => "admin",
            V4Role::Deployer => "deployer",
            V4Role::ReadOnly => "read-only",
        };
        stable_id(role, self.instance_id, Some("token"))
    }
}

/// Discover every explicitly registered instance for the daemon-wide admin token.
/// The Cloudflare account naming stays confined to this wire boundary.
pub(crate) fn shared_discovery(
    path: &str,
    raw_query: Option<&str>,
    views: Vec<InstanceView>,
    role: V4Role,
) -> Option<Response> {
    let kind = match path {
        "/client/v4/accounts" => CollectionKind::Accounts,
        "/client/v4/memberships" => CollectionKind::Memberships,
        _ => return None,
    };
    let context = V4RequestContext {
        role,
        request_id: RequestId::generate(),
    };
    let query = match CollectionQuery::parse(raw_query, kind) {
        Ok(query) => query,
        Err(error) => return Some(error_response(error, context.request_id())),
    };
    let mut accounts = views
        .into_iter()
        .map(|view| Account {
            name: view.name.unwrap_or_else(|| view.instance_id.clone()),
            id: view.instance_id,
            kind: "standard",
            created_on: None,
        })
        .filter(|account| {
            query
                .name
                .as_deref()
                .is_none_or(|name| name == account.name)
        })
        .collect::<Vec<_>>();
    accounts.sort_by(|a, b| {
        let order = match query.order {
            CollectionOrder::Id | CollectionOrder::Status => a.id.cmp(&b.id),
            CollectionOrder::AccountName => a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)),
        };
        if query.descending {
            order.reverse()
        } else {
            order
        }
    });
    let total = accounts.len();
    let offset = query.page.saturating_sub(1).saturating_mul(query.per_page);
    let selected = accounts
        .into_iter()
        .skip(offset)
        .take(query.per_page)
        .collect::<Vec<_>>();
    Some(match kind {
        CollectionKind::Accounts => {
            let count = selected.len();
            success_collection(context, selected, &query, count, total)
        }
        CollectionKind::Memberships => {
            let memberships = selected
                .into_iter()
                .map(|account| Membership {
                    id: account.id.clone(),
                    account,
                    api_access_enabled: true,
                    roles: vec![role_name(role)],
                    status: "accepted",
                })
                .collect::<Vec<_>>();
            let count = memberships.len();
            success_collection(context, memberships, &query, count, total)
        }
    })
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
        Err(response) => return response.into_response(),
    };
    let Some(account) = state.v4_instance_context() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    success_response(
        context,
        User {
            id: stable_id("user", account.instance_id, None),
            email: USER_EMAIL,
        },
    )
}

async fn verify_token(State(state): State<HttpState>, request: Request) -> Response {
    let context = match read_context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let Some(account) = state.v4_instance_context() else {
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
        Err(response) => return response.into_response(),
    };
    let query = match CollectionQuery::parse(request.uri().query(), CollectionKind::Accounts) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(authority) = state.v4_instance_context() else {
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
        &query,
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
        Err(response) => return response.into_response(),
    };
    let Some(authority) = state.v4_instance_context() else {
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
        Err(response) => return response.into_response(),
    };
    let query = match CollectionQuery::parse(request.uri().query(), CollectionKind::Memberships) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(authority) = state.v4_instance_context() else {
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
        id: stable_id("membership", authority.instance_id, None),
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
        &query,
        visible,
        usize::from(matches),
    )
}

fn read_context(
    request: &Request,
    permission: V4Permission,
) -> Result<V4RequestContext, HttpError> {
    let context = request_context(request)?;
    context
        .require(permission)
        .map_err(|error| HttpError::from_response(error_response(error, context.request_id())))?;
    Ok(context)
}

fn success_collection<T: Serialize>(
    context: V4RequestContext,
    result: Vec<T>,
    query: &CollectionQuery,
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
            total_pages: total_count.div_ceil(query.per_page),
        },
    )
}

#[derive(Clone, Copy)]
enum CollectionKind {
    Accounts,
    Memberships,
}

#[derive(Clone, Copy)]
enum CollectionOrder {
    Id,
    AccountName,
    Status,
}

struct CollectionQuery {
    page: usize,
    per_page: usize,
    name: Option<String>,
    descending: bool,
    order: CollectionOrder,
}

impl CollectionQuery {
    fn parse(raw: Option<&str>, kind: CollectionKind) -> Result<Self, V4Error> {
        let mut page = 1;
        let mut per_page = 20;
        let mut name = None;
        let mut descending = false;
        let mut order = CollectionOrder::Id;
        for (key, value) in form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
            match key.as_ref() {
                "page" => page = parse_usize(&value)?,
                "per_page" => {
                    per_page = parse_usize(&value)?;
                    if !(5..=50).contains(&per_page) {
                        return Err(V4Error::InvalidField("/per_page"));
                    }
                }
                "name" if matches!(kind, CollectionKind::Accounts) => {
                    name = one(name.is_some(), &value)?;
                }
                "account.name" | "name" if matches!(kind, CollectionKind::Memberships) => {
                    name = one(name.is_some(), &value)?;
                }
                "direction" if value == "asc" => descending = false,
                "direction" if value == "desc" => descending = true,
                "order" if matches!(kind, CollectionKind::Memberships) => {
                    order = match value.as_ref() {
                        "id" => CollectionOrder::Id,
                        "account.name" => CollectionOrder::AccountName,
                        "status" => CollectionOrder::Status,
                        _ => return Err(V4Error::InvalidRequest),
                    };
                }
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
            descending,
            order,
        })
    }
}

fn one(has_existing: bool, value: &str) -> Result<Option<String>, V4Error> {
    if has_existing || value.is_empty() || value.len() > 100 {
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

fn stable_id(scope: &str, platform: InstanceId, suffix: Option<&str>) -> String {
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
    name: String,
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_on: Option<String>,
}

#[derive(Serialize)]
struct Membership {
    id: String,
    account: Account,
    api_access_enabled: bool,
    roles: Vec<&'static str>,
    status: &'static str,
}
