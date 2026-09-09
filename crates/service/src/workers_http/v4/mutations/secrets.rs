use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SecretBody {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    text: Option<SecretString>,
    format: Option<String>,
    algorithm: Option<serde_json::Value>,
    usages: Option<Vec<String>>,
    key_base64: Option<SecretString>,
    key_jwk: Option<serde_json::Value>,
}

impl SecretBody {
    pub(super) fn text(self) -> Result<(String, SecretString), V4Error> {
        if self.kind == "secret_key" {
            return Err(V4Error::Unsupported);
        }
        if self.kind != "secret_text"
            || self.format.is_some()
            || self.algorithm.is_some()
            || self.usages.is_some()
            || self.key_base64.is_some()
            || self.key_jwk.is_some()
        {
            return Err(V4Error::InvalidRequest);
        }
        Ok((self.name, self.text.ok_or(V4Error::InvalidRequest)?))
    }
}

#[derive(Clone, Serialize)]
struct SecretItem {
    name: String,
    #[serde(rename = "type")]
    kind: &'static str,
}

pub(in crate::workers_http::v4) async fn list_secrets(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let result = active_snapshot(&state, &account, &script).map(|(_, snapshot)| {
        snapshot
            .secrets
            .keys()
            .map(|name| SecretItem {
                name: name.clone(),
                kind: "secret_text",
            })
            .collect::<Vec<_>>()
    });
    respond(context, result)
}

pub(in crate::workers_http::v4) async fn put_secret(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match authorize(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let secret = match json_body::<SecretBody>(request)
        .await
        .and_then(SecretBody::text)
    {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let item = SecretItem {
        name: secret.0.clone(),
        kind: "secret_text",
    };
    let mut updates = BTreeMap::new();
    updates.insert(secret.0, Some(secret.1));
    match mutate(
        &state,
        &account,
        &script,
        updates,
        None,
        context.request_id(),
    )
    .await
    {
        Ok(()) => success_response(context, item),
        Err(error) => platform_error(context.request_id(), &error),
    }
}

pub(in crate::workers_http::v4) async fn get_secret(
    State(state): State<HttpState>,
    Path((account, script, secret)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let context = match authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let result = active_snapshot(&state, &account, &script).and_then(|(_, snapshot)| {
        snapshot
            .secrets
            .contains_key(&secret)
            .then_some(SecretItem {
                name: secret,
                kind: "secret_text",
            })
            .ok_or(V4Error::NotFound)
    });
    respond(context, result)
}

pub(in crate::workers_http::v4) async fn delete_secret(
    State(state): State<HttpState>,
    Path((account, script, secret)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let context = match authorize(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let mut updates = BTreeMap::new();
    updates.insert(secret, None);
    match mutate(
        &state,
        &account,
        &script,
        updates,
        None,
        context.request_id(),
    )
    .await
    {
        Ok(()) => success_response(context, ()),
        Err(error) => platform_error(context.request_id(), &error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretBulkPatch {
    secrets: BTreeMap<String, Option<SecretBody>>,
    version_tags: Option<BTreeMap<String, serde_json::Value>>,
}

pub(in crate::workers_http::v4) async fn patch_secrets_bulk(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match authorize(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let body = match json_body::<SecretBulkPatch>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    if body
        .version_tags
        .as_ref()
        .is_some_and(|tags| !tags.is_empty())
    {
        return error_response(V4Error::Unsupported, context.request_id());
    }
    let mut updates = BTreeMap::new();
    for (map_name, value) in body.secrets {
        let value = match value {
            Some(value) => match value.text() {
                Ok((body_name, text)) if body_name == map_name => Some(text),
                Ok(_) => {
                    return error_response(V4Error::InvalidRequest, context.request_id());
                }
                Err(error) => return error_response(error, context.request_id()),
            },
            None => None,
        };
        updates.insert(map_name, value);
    }
    match mutate(
        &state,
        &account,
        &script,
        updates,
        None,
        context.request_id(),
    )
    .await
    {
        Ok(()) => match active_snapshot(&state, &account, &script) {
            Ok((_, snapshot)) => success_response(
                context,
                snapshot
                    .secrets
                    .keys()
                    .map(|name| {
                        (
                            name.clone(),
                            SecretItem {
                                name: name.clone(),
                                kind: "secret_text",
                            },
                        )
                    })
                    .collect::<BTreeMap<_, _>>(),
            ),
            Err(error) => error_response(error, context.request_id()),
        },
        Err(error) => platform_error(context.request_id(), &error),
    }
}
