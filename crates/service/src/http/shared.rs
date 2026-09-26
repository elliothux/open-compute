//! One HTTP listener dispatching only to explicitly running instances.

use super::{HttpState, Router};
use crate::cloudflare_v4::V4Role;
use crate::metrics::MetricSeriesBudget;
use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use open_compute_core::{ErrorCode, InstanceId, PlatformError, RequestId, SecretString, StartupId};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use tower::ServiceExt;

#[derive(Clone)]
pub(crate) struct SharedRoutes {
    inner: Arc<RwLock<HashMap<InstanceId, InstanceRoutes>>>,
    daemon: Option<crate::run::daemon_control::DaemonApi>,
    dashboard_auth: Arc<crate::dashboard_auth::DashboardAuth>,
    metrics: Arc<MetricSeriesBudget>,
}

struct InstanceRoutes {
    generation: StartupId,
    dashboard_enabled: bool,
    public: Router,
    admin: Router,
    gateway: Option<Router>,
    public_base_domain: Option<String>,
    admin_secret: Arc<SecretString>,
    deployer_secret: Option<Arc<SecretString>>,
    read_only_secret: Option<Arc<SecretString>>,
}

pub(crate) struct RouteLease {
    routes: SharedRoutes,
    instance_id: InstanceId,
    generation: StartupId,
}

impl SharedRoutes {
    pub(crate) fn new(
        daemon: Option<crate::run::daemon_control::DaemonApi>,
        max_series: u64,
    ) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            daemon,
            dashboard_auth: Arc::new(crate::dashboard_auth::DashboardAuth::new(
                StartupId::generate(),
            )),
            metrics: Arc::new(MetricSeriesBudget::new(max_series)),
        }
    }

    pub(crate) fn dashboard_auth(&self) -> Arc<crate::dashboard_auth::DashboardAuth> {
        self.dashboard_auth.clone()
    }

    pub(crate) fn insert(
        &self,
        instance_id: InstanceId,
        state: HttpState,
        public_base_domain: Option<&str>,
    ) -> Result<RouteLease, PlatformError> {
        if let Some(domain) = public_base_domain {
            open_compute_core::PublicGatewayConfig::validate_base_domain(domain)?;
        }
        let admin_secret = state.admin_secret.clone().ok_or_else(invalid_routes)?;
        if let Some(daemon) = &self.daemon {
            let deployer = state
                .deployer_secret
                .as_deref()
                .ok_or_else(invalid_routes)?;
            let read_only = state
                .read_only_secret
                .as_deref()
                .ok_or_else(invalid_routes)?;
            if !daemon.matches_runtime(
                &instance_id,
                &admin_secret,
                deployer,
                read_only,
                public_base_domain,
            )? {
                return Err(PlatformError::new(
                    ErrorCode::SecretRefInvalid,
                    "instance runtime authority differs from registered intent",
                ));
            }
        }
        let mut entries = self.inner.write().map_err(|_| invalid_routes())?;
        if entries.contains_key(&instance_id) {
            return Err(invalid_routes());
        }
        for entry in entries.values() {
            if entry.admin_secret.expose() != admin_secret.expose() || tokens_overlap(&state, entry)
            {
                return Err(PlatformError::new(
                    ErrorCode::SecretRefInvalid,
                    "registered instance Bearer tokens conflict",
                ));
            }
            if let (Some(domain), Some(existing)) =
                (public_base_domain, entry.public_base_domain.as_deref())
                && crate::instance_registry::public_domains_overlap(domain, existing)
            {
                return Err(invalid_routes());
            }
        }
        let generation = StartupId::generate();
        entries.insert(
            instance_id,
            InstanceRoutes {
                generation,
                dashboard_enabled: state.dashboard_enabled,
                public: super::public_router(state.clone()),
                admin: super::admin_router(state.clone()),
                gateway: public_base_domain.map(|_| super::gateway_router(state.clone())),
                public_base_domain: public_base_domain.map(str::to_owned),
                admin_secret,
                deployer_secret: state.deployer_secret,
                read_only_secret: state.read_only_secret,
            },
        );
        Ok(RouteLease {
            routes: self.clone(),
            instance_id,
            generation,
        })
    }

    pub(crate) fn router(&self, admin_allowed: bool, listener_port: u16) -> Router {
        let routes = self.clone();
        Router::new()
            .route("/health/live", get(|| async { StatusCode::OK }))
            .route("/health/ready", get(|| async { StatusCode::OK }))
            .fallback(move |request: Request| {
                let routes = routes.clone();
                async move { routes.dispatch(request, admin_allowed, listener_port).await }
            })
    }

    pub(crate) fn gateway_router(&self) -> Router {
        let routes = self.clone();
        Router::new()
            .route(
                "/__open_compute_gateway_probe__",
                get(|| async {
                    (
                        StatusCode::NO_CONTENT,
                        [("x-open-compute-gateway-probe", "1")],
                    )
                }),
            )
            .fallback(move |request: Request| {
                let routes = routes.clone();
                async move { routes.dispatch_gateway(request).await }
            })
    }

    async fn dispatch_gateway(&self, request: Request) -> Response {
        let Some(hostname) = request
            .headers()
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| crate::workers_http::canonical_request_host(value, Some(443)).ok())
        else {
            return StatusCode::NOT_FOUND.into_response();
        };
        match self.gateway_route(&hostname) {
            Ok(Some((_, router))) => router
                .oneshot(request)
                .await
                .unwrap_or_else(|_| StatusCode::SERVICE_UNAVAILABLE.into_response()),
            Ok(None) => StatusCode::NOT_FOUND.into_response(),
            Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        }
    }

    fn gateway_route(&self, hostname: &str) -> Result<Option<(InstanceId, Router)>, PlatformError> {
        let entries = self.inner.read().map_err(|_| invalid_routes())?;
        let mut matched = entries.iter().filter(|(_, entry)| {
            entry.public_base_domain.as_deref().is_some_and(|domain| {
                hostname
                    .strip_suffix(domain)
                    .is_some_and(|prefix| prefix.ends_with('.') && prefix.len() > 1)
            })
        });
        let Some((id, entry)) = matched.next() else {
            return Ok(None);
        };
        if matched.next().is_some() {
            return Err(invalid_routes());
        }
        Ok(entry.gateway.clone().map(|router| (*id, router)))
    }

    async fn dispatch(
        &self,
        request: Request,
        admin_allowed: bool,
        listener_port: u16,
    ) -> Response {
        let Some(host) = request
            .headers()
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| shared_request_host(value, listener_port))
        else {
            return StatusCode::NOT_FOUND.into_response();
        };
        if admin_allowed
            && matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]")
            && request.uri().path().starts_with("/operator/api/instances")
        {
            return self.operator_request(request, &host, admin_allowed).await;
        }
        if admin_allowed
            && matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]")
            && matches!(
                request.uri().path(),
                "/client/v4/accounts" | "/client/v4/memberships"
            )
            && let Some(daemon) = &self.daemon
        {
            let bearer = request
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok());
            let visible = if crate::auth::bearer_token(bearer).is_some_and(|token| {
                self.dashboard_auth
                    .session_valid(token, std::time::SystemTime::now())
            }) {
                daemon.list().map(|views| Some((views, V4Role::Admin)))
            } else {
                daemon.visible_for_bearer(bearer)
            };
            let Ok(visible) = visible else {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            };
            let Some((views, role)) = visible else {
                return crate::cloudflare_v4::error_response(
                    crate::cloudflare_v4::V4Error::AuthenticationRequired,
                    RequestId::generate(),
                );
            };
            if request.method() != Method::GET {
                return StatusCode::METHOD_NOT_ALLOWED.into_response();
            }
            return crate::cloudflare_v4::accounts::shared_discovery(
                request.uri().path(),
                request.uri().query(),
                views,
                role,
            )
            .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response());
        }
        let local = host.ends_with(".localhost");
        let shared_session = matches!(
            request.uri().path(),
            "/operator/session" | "/operator/session/exchange"
        );
        let dashboard_path = request.uri().path();
        let dashboard_shell = admin_allowed
            && request.method() == Method::GET
            && (dashboard_path == "/operator" || dashboard_path.starts_with("/operator/"))
            && dashboard_path != "/operator/api"
            && !dashboard_path.starts_with("/operator/api/");
        let target = if shared_session {
            let Ok(entries) = self.inner.read() else {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            };
            entries
                .keys()
                .min_by(|left, right| left.as_str().cmp(right.as_str()))
                .copied()
                .map(|id| (id, false))
        } else if local {
            let Some(target) = local_origin_instance(&host) else {
                return StatusCode::NOT_FOUND.into_response();
            };
            Some(target)
        } else if dashboard_shell {
            let Ok(entries) = self.inner.read() else {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            };
            entries
                .iter()
                .filter(|(_, entry)| entry.dashboard_enabled)
                .min_by(|(left, _), (right, _)| left.as_str().cmp(right.as_str()))
                .map(|(id, _)| (*id, false))
        } else {
            account_path_instance(request.uri().path())
                .or_else(|| git_path_instance(request.uri().path()))
                .or_else(|| signed_tail_path_instance(request.uri().path()))
                .map(|id| (id, false))
        };
        let target = if target.is_none() {
            if let Some(daemon) = &self.daemon {
                let bearer = request
                    .headers()
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok());
                let visible = daemon.visible_for_bearer(bearer);
                let Ok(visible) = visible else {
                    return StatusCode::SERVICE_UNAVAILABLE.into_response();
                };
                let Some((views, _)) = visible else {
                    return StatusCode::NOT_FOUND.into_response();
                };
                let [view] = views.as_slice() else {
                    return StatusCode::NOT_FOUND.into_response();
                };
                let Ok(id) = InstanceId::from_str(&view.instance_id) else {
                    return StatusCode::SERVICE_UNAVAILABLE.into_response();
                };
                Some((id, false))
            } else {
                None
            }
        } else {
            target
        };
        let requested_metrics = request.uri().path() == "/metrics";
        let (router, instance_id, generation, metric_request) = {
            let Ok(entries) = self.inner.read() else {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            };
            let (instance_id, worker_origin) = match target {
                Some(target) => target,
                None => {
                    let bearer = request
                        .headers()
                        .get(header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok());
                    let mut matched = entries.iter().filter(|(_, entry)| {
                        crate::auth::bearer_matches(bearer, &entry.admin_secret)
                            || entry
                                .deployer_secret
                                .as_deref()
                                .is_some_and(|token| crate::auth::bearer_matches(bearer, token))
                            || entry
                                .read_only_secret
                                .as_deref()
                                .is_some_and(|token| crate::auth::bearer_matches(bearer, token))
                    });
                    let Some((id, _)) = matched.next() else {
                        return StatusCode::NOT_FOUND.into_response();
                    };
                    if matched.next().is_some() {
                        return StatusCode::NOT_FOUND.into_response();
                    }
                    (*id, false)
                }
            };
            let Some(entry) = entries.get(&instance_id) else {
                return StatusCode::NOT_FOUND.into_response();
            };
            let admin_route =
                admin_allowed && !worker_origin && !request.uri().path().starts_with("/git/");
            let router = if admin_route {
                entry.admin.clone()
            } else {
                entry.public.clone()
            };
            (
                router,
                instance_id,
                entry.generation,
                admin_route && requested_metrics,
            )
        };
        let response = router
            .oneshot(request)
            .await
            .unwrap_or_else(|_| StatusCode::SERVICE_UNAVAILABLE.into_response());
        if !metric_request || response.status() != StatusCode::OK {
            return response;
        }
        let (mut parts, body) = response.into_parts();
        let Ok(bytes) = to_bytes(body, usize::MAX).await else {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        };
        let Ok(rendered) = std::str::from_utf8(&bytes) else {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        };
        let Ok(entries) = self.inner.read() else {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        };
        if !entries
            .get(&instance_id)
            .is_some_and(|entry| entry.generation == generation)
        {
            return StatusCode::NOT_FOUND.into_response();
        }
        let Ok(scoped) = self.metrics.render(instance_id, rendered) else {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "metric series capacity unavailable\n",
            )
                .into_response();
        };
        parts.headers.remove(header::CONTENT_LENGTH);
        Response::from_parts(parts, Body::from(scoped))
    }

    async fn operator_request(
        &self,
        request: Request,
        host: &str,
        admin_allowed: bool,
    ) -> Response {
        if !admin_allowed || !matches!(host, "localhost" | "127.0.0.1" | "[::1]") {
            return StatusCode::NOT_FOUND.into_response();
        }
        let Some(daemon) = &self.daemon else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let bearer = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok());
        let session_authorized = crate::auth::bearer_token(bearer).is_some_and(|token| {
            self.dashboard_auth
                .session_valid(token, std::time::SystemTime::now())
        });
        if !daemon.authorized(bearer) && !session_authorized {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        let path = request.uri().path();
        if request.method() == Method::GET && path == "/operator/api/instances" {
            return match daemon.list() {
                Ok(instances) => {
                    axum::Json(serde_json::json!({ "instances": instances })).into_response()
                }
                Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
            };
        }
        if request.method() != Method::POST {
            return StatusCode::NOT_FOUND.into_response();
        }
        let Some(rest) = path.strip_prefix("/operator/api/instances/") else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let Some((id, operation)) = rest.split_once('/') else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let Ok(instance_id) = InstanceId::from_str(id) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let action = match operation {
            "start" => crate::run::daemon_control::LifecycleAction::Start,
            "stop" => crate::run::daemon_control::LifecycleAction::Stop,
            _ => return StatusCode::NOT_FOUND.into_response(),
        };
        match daemon.request(action, instance_id).await {
            Ok(()) => StatusCode::ACCEPTED.into_response(),
            Err(error) if error.code() == ErrorCode::InstanceNotFound => {
                StatusCode::NOT_FOUND.into_response()
            }
            Err(error) if error.code() == ErrorCode::InstanceRegistryInvalid => {
                StatusCode::CONFLICT.into_response()
            }
            Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        }
    }

    fn remove(&self, instance_id: &InstanceId, generation: StartupId) {
        if let Ok(mut entries) = self.inner.write()
            && entries
                .get(instance_id)
                .is_some_and(|entry| entry.generation == generation)
        {
            entries.remove(instance_id);
            self.metrics.remove(instance_id);
        }
    }
}

fn shared_request_host(value: &str, listener_port: u16) -> Option<String> {
    let authority = value.parse::<axum::http::uri::Authority>().ok()?;
    if authority
        .port_u16()
        .is_some_and(|port| port != listener_port)
    {
        return None;
    }
    match authority.host() {
        "localhost" | "127.0.0.1" | "[::1]" => Some(authority.host().to_owned()),
        _ => crate::workers_http::canonical_request_host(value, Some(listener_port)).ok(),
    }
}

impl RouteLease {
    pub(crate) fn withdraw(&self) {
        self.routes.remove(&self.instance_id, self.generation);
    }
}

impl Drop for RouteLease {
    fn drop(&mut self) {
        self.withdraw();
    }
}

fn local_origin_instance(host: &str) -> Option<(InstanceId, bool)> {
    let labels = host
        .strip_suffix(".localhost")?
        .split('.')
        .collect::<Vec<_>>();
    match labels.as_slice() {
        [id] => Some((InstanceId::from_str(id).ok()?, false)),
        [worker, id] if !worker.is_empty() => Some((InstanceId::from_str(id).ok()?, true)),
        _ => None,
    }
}

fn account_path_instance(path: &str) -> Option<InstanceId> {
    let mut segments = path.strip_prefix("/client/v4/accounts/")?.split('/');
    InstanceId::from_str(segments.next()?).ok()
}

fn git_path_instance(path: &str) -> Option<InstanceId> {
    let mut segments = path.strip_prefix("/git/")?.split('/');
    InstanceId::from_str(segments.next()?).ok()
}

fn signed_tail_path_instance(path: &str) -> Option<InstanceId> {
    let mut segments = path.strip_prefix("/client/v4/open-compute/")?.split('/');
    match segments.next()? {
        "tails" | "live-tails" => InstanceId::from_str(segments.next()?).ok(),
        _ => None,
    }
}

fn tokens_overlap(state: &HttpState, entry: &InstanceRoutes) -> bool {
    let new_tokens = [
        state.deployer_secret.as_deref(),
        state.read_only_secret.as_deref(),
    ];
    let old_tokens = [
        Some(entry.admin_secret.as_ref()),
        entry.deployer_secret.as_deref(),
        entry.read_only_secret.as_deref(),
    ];
    new_tokens.into_iter().flatten().any(|token| {
        old_tokens
            .into_iter()
            .flatten()
            .any(|old| token.expose() == old.expose())
    })
}

fn invalid_routes() -> PlatformError {
    PlatformError::new(
        ErrorCode::InstanceRegistryInvalid,
        "instance HTTP route registry is unavailable or duplicate",
    )
}

#[cfg(test)]
#[path = "shared_tests.rs"]
mod tests;
