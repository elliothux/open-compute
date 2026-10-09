//! Signed, generation-fenced viewer capabilities; browser credentials never enter the frontend.

use super::*;
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderValue, Method, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::Mac as _;
use serde::{Deserialize, Serialize};

type Signer = hmac::Hmac<sha2::Sha256>;
const HEADER: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9";

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Mode {
    #[default]
    Devtools,
    Tab,
    Full,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Guardrails {
    mode: GuardrailMode,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum GuardrailMode {
    Readonly,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Options {
    mode: Option<Mode>,
    expires_in_ms: Option<u64>,
    target_id: Option<String>,
    guardrails: Option<Guardrails>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Claims {
    instance: InstanceId,
    generation: String,
    session: String,
    target: String,
    mode: Mode,
    readonly: bool,
    expires_at_ms: i64,
    exp: i64,
    jti: String,
}

impl BrowserService {
    pub(crate) async fn live_view(
        &self,
        id: &str,
        bytes: &[u8],
        origin: &str,
    ) -> Result<Value, PlatformError> {
        let options: Options = if bytes.is_empty() {
            Options::default()
        } else {
            serde_json::from_slice(bytes).map_err(|_| invalid())?
        };
        let _capacity = self
            .connections
            .clone()
            .try_acquire_owned()
            .map_err(|_| limit())?;
        self.live_view_options(id, options, origin).await
    }

    pub(super) async fn cdp_live_view(
        &self,
        id: &str,
        cdp: &BrowserCdp,
        params: Value,
        attachment: Option<&str>,
        current_target: Option<&str>,
    ) -> Result<Value, PlatformError> {
        // CDP cannot mint REST-only guardrails or choose a frontend origin.
        if params.get("guardrails").is_some() {
            return Err(invalid());
        }
        let mut options: Options = serde_json::from_value(params).map_err(|_| invalid())?;
        let (_, origin) = self.public_urls(id, None)?;
        let target = if let Some(attachment) = attachment {
            let reply = cdp
                .command("Target.getTargetInfo", json!({}), Some(attachment))
                .await
                .map_err(|_| not_found())?;
            Some(
                reply
                    .pointer("/result/targetInfo/targetId")
                    .and_then(Value::as_str)
                    .ok_or_else(not_found)?
                    .to_owned(),
            )
        } else {
            current_target.map(str::to_owned)
        };
        if options.target_id.is_none() {
            options.target_id = target;
        }
        let view = self.live_view_options(id, options, &origin).await?;
        Ok(json!({"devtoolsFrontendUrl":view["devtoolsFrontendUrl"]}))
    }

    async fn live_view_options(
        &self,
        id: &str,
        options: Options,
        origin: &str,
    ) -> Result<Value, PlatformError> {
        let session = self.session(id)?;
        let reply = Box::pin(self.command(
            id,
            &session.cdp,
            json!({"id":0,"method":"Target.getTargets"}),
            None,
        ))
        .await?;
        let targets = reply
            .pointer("/result/targetInfos")
            .ok_or_else(backend::unavailable)?;
        let target = targets
            .as_array()
            .ok_or_else(backend::unavailable)?
            .iter()
            .find(|row| {
                row["type"] == "page"
                    && row["targetId"].as_str().is_some_and(|target| {
                        options
                            .target_id
                            .as_ref()
                            .is_none_or(|expected| expected == target)
                    })
            })
            .and_then(|row| row["targetId"].as_str())
            .ok_or_else(not_found)?;
        self.mint_view(id, target, options, origin)
    }

    pub(crate) fn default_view(
        &self,
        id: &str,
        target: &str,
        origin: &str,
        readonly: bool,
    ) -> Result<Value, PlatformError> {
        self.mint_view(
            id,
            target,
            Options {
                guardrails: readonly.then_some(Guardrails {
                    mode: GuardrailMode::Readonly,
                }),
                ..Options::default()
            },
            origin,
        )
    }

    fn mint_view(
        &self,
        id: &str,
        target: &str,
        options: Options,
        origin: &str,
    ) -> Result<Value, PlatformError> {
        self.session(id)?;
        let ttl = options.expires_in_ms.unwrap_or(300_000);
        if !(60_000..=3_600_000).contains(&ttl) {
            return Err(invalid());
        }
        let mode = options.mode.unwrap_or_default();
        let expires_at_ms = now_ms().checked_add(ttl as i64).ok_or_else(invalid)?;
        let claims = Claims {
            instance: self.instance,
            generation: self.generation.clone(),
            session: id.into(),
            target: target.into(),
            mode,
            readonly: options.guardrails.is_some(),
            expires_at_ms,
            exp: (expires_at_ms + 999) / 1000,
            jti: uuid::Uuid::now_v7().to_string(),
        };
        let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).map_err(|_| invalid())?);
        let message = format!("{HEADER}.{encoded}");
        let mut signer =
            Signer::new_from_slice(self.live_key.as_ref()).map_err(|_| backend::unavailable())?;
        signer.update(message.as_bytes());
        let token = format!(
            "{message}.{}",
            URL_SAFE_NO_PAD.encode(signer.finalize().into_bytes())
        );
        let mut frontend = url::Url::parse(origin)
            .map_err(|_| invalid())?
            .join("view")
            .map_err(|_| invalid())?;
        frontend.set_fragment(Some(&format!("jwt={token}")));
        let mut websocket = url::Url::parse(origin)
            .map_err(|_| invalid())?
            .join(&format!("{id}/page/{target}"))
            .map_err(|_| invalid())?;
        websocket
            .set_scheme(if websocket.scheme() == "https" {
                "wss"
            } else {
                "ws"
            })
            .map_err(|()| invalid())?;
        websocket.query_pairs_mut().append_pair("jwt", &token);
        let mut view_options = json!({"mode":mode});
        if let Some(guardrails) = options.guardrails {
            view_options["guardrails"] = serde_json::to_value(guardrails).map_err(|_| invalid())?;
        }
        Ok(
            json!({"id":target,"options":view_options,"devtoolsFrontendUrl":frontend.as_str(),"webSocketDebuggerUrl":websocket.as_str()}),
        )
    }

    fn claim(&self, token: &str, id: &str, deadline: bool) -> Result<Claims, PlatformError> {
        if token.len() > 4096 {
            return Err(not_found());
        }
        let fields = token.split('.').collect::<Vec<_>>();
        let [header, payload, signature] = fields.as_slice() else {
            return Err(not_found());
        };
        if *header != HEADER {
            return Err(not_found());
        }
        let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| not_found())?;
        let mut signer =
            Signer::new_from_slice(self.live_key.as_ref()).map_err(|_| backend::unavailable())?;
        signer.update(format!("{header}.{payload}").as_bytes());
        signer.verify_slice(&signature).map_err(|_| not_found())?;
        let bytes = URL_SAFE_NO_PAD.decode(payload).map_err(|_| not_found())?;
        let claims: Claims = serde_json::from_slice(&bytes).map_err(|_| not_found())?;
        if claims.instance != self.instance
            || claims.generation != self.generation
            || claims.session != id
            || deadline && claims.expires_at_ms <= now_ms()
        {
            return Err(not_found());
        }
        self.session(id)?;
        Ok(claims)
    }
}

pub(crate) fn router() -> Router<crate::http::HttpState> {
    Router::new()
        .route(
            "/accounts/{account_id}/browser-rendering/live/view",
            get(view),
        )
        .route(
            "/accounts/{account_id}/browser-rendering/live/view.js",
            get(script),
        )
        .route(
            "/accounts/{account_id}/browser-rendering/live/{session}/targets",
            get(targets),
        )
        .route(
            "/accounts/{account_id}/browser-rendering/live/{session}/page/{target}",
            get(page),
        )
        .route(
            "/accounts/{account_id}/browser-rendering/live/{session}/devtools/{*asset}",
            get(asset),
        )
}
fn service(
    state: &crate::http::HttpState,
    account: &str,
) -> Result<Arc<BrowserService>, PlatformError> {
    state
        .browser_service()
        .filter(|service| service.is_available() && service.instance.as_str() == account)
        .cloned()
        .ok_or_else(backend::unavailable)
}
fn token(request: &Request) -> Result<String, PlatformError> {
    let query = crate::cloudflare_v4::strict_query(request).map_err(|_| invalid())?;
    if query.len() != 1 {
        return Err(invalid());
    }
    query.get("jwt").cloned().ok_or_else(not_found)
}
async fn view(
    State(state): State<crate::http::HttpState>,
    Path(account): Path<String>,
) -> Response {
    if let Err(error) = service(&state, &account) {
        return http::error(&error);
    }
    bounded_response(
        Body::from(include_str!("live.html")),
        "text/html; charset=utf-8",
    )
}
async fn script(
    State(state): State<crate::http::HttpState>,
    Path(account): Path<String>,
) -> Response {
    if let Err(error) = service(&state, &account) {
        return http::error(&error);
    }
    match open_compute_runtime::embedded_browser_view() {
        Some(bytes) => bounded_response(Body::from(bytes), "text/javascript; charset=utf-8"),
        None => http::error(&backend::unavailable()),
    }
}
fn bounded_response(body: Body, media: &str) -> Response {
    let mut response = Response::new(body);
    let headers = response.headers_mut();
    if let Ok(media) = HeaderValue::from_str(media) {
        headers.insert(header::CONTENT_TYPE, media);
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("content-security-policy", HeaderValue::from_static("default-src 'self'; script-src 'self' 'unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; worker-src 'self' blob:; frame-src 'self'; base-uri 'self'; form-action 'none'; frame-ancestors 'self'"));
    response
}
async fn targets(
    State(state): State<crate::http::HttpState>,
    Path((account, id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let result = async move {
        let service = service(&state, &account)?;
        let claims = service.claim(&token(&request)?, &id, true)?;
        if claims.mode != Mode::Full {
            return Err(not_found());
        }
        let value = service
            .devtools(&id, "json/list", &Method::GET, &BTreeMap::new())
            .await?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(|error: PlatformError| http::error(&error))
}
async fn page(
    State(state): State<crate::http::HttpState>,
    Path((account, id, target)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let result = async move {
        let service = service(&state, &account)?;
        let claims = service.claim(&token(&request)?, &id, true)?;
        if claims.mode != Mode::Full && claims.target != target {
            return Err(not_found());
        }
        websocket::upgrade_live(service, &id, &target, claims.readonly, request).await
    }
    .await;
    result.unwrap_or_else(|error: PlatformError| http::error(&error))
}
async fn asset(
    State(state): State<crate::http::HttpState>,
    Path((account, id, asset)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let result = async move {
        let service = service(&state, &account)?;
        let cookie_name = format!("oc_browser_view_{id}");
        let inspector = asset == "inspector.html";
        let jwt = if inspector {
            let query = crate::cloudflare_v4::strict_query(&request).map_err(|_| invalid())?;
            if query.keys().any(|key| !matches!(key.as_str(), "jwt" | "ws" | "wss")) { return Err(invalid()); }
            query.get("jwt").cloned().ok_or_else(not_found)?
        } else {
            if request.uri().query().is_some() { return Err(invalid()); }
            request.headers().get(header::COOKIE).and_then(|value| value.to_str().ok()).and_then(|value| value.split(';').find_map(|field| { let (name, value) = field.trim().split_once('=')?; (name == cookie_name).then(|| value.to_owned()) })).ok_or_else(not_found)?
        };
        let claims = service.claim(&jwt, &id, inspector)?;
        if claims.mode != Mode::Devtools { return Err(not_found()); }
        if inspector {
            let query = crate::cloudflare_v4::strict_query(&request).map_err(|_| invalid())?;
            let (_, origin) = service.public_urls(&id, state.control_origin_port())?;
            let mut endpoint = url::Url::parse(&origin).map_err(|_| invalid())?
                .join(&format!("{id}/page/{}", claims.target)).map_err(|_| invalid())?;
            endpoint.query_pairs_mut().append_pair("jwt", &jwt);
            let key = if endpoint.scheme() == "https" { "wss" } else { "ws" };
            let expected = endpoint.as_str().split_once("://").ok_or_else(invalid)?.1;
            if query.len() != 2 || query.get(key).map(String::as_str) != Some(expected) { return Err(invalid()); }
        }
        let _request = service.frontend_capacity.try_acquire().map_err(|_| limit())?;
        let _capacity = tokio::time::timeout(Duration::from_millis(service.config.command_timeout_ms), service.connections.clone().acquire_owned()).await.map_err(|_| timeout())?.map_err(|_| backend::unavailable())?;
        let (bytes, media) = service.frontend(&id, &asset).await?;
        service.session(&id)?;
        let mut response = bounded_response(Body::from(bytes), &media);
        if inspector {
            let secure = if service.public_origin.as_ref().is_some_and(|origin| origin.scheme() == "https") { "; Secure" } else { "" };
            let cookie = format!("{cookie_name}={jwt}; Path=/client/v4/accounts/{account}/browser-rendering/live/{id}/devtools/; HttpOnly; SameSite=Strict{secure}");
            response.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).map_err(|_| invalid())?);
        }
        Ok(response)
    }.await;
    result.unwrap_or_else(|error: PlatformError| http::error(&error))
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;
