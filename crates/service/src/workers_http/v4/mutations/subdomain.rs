use super::*;

#[derive(Serialize)]
struct Subdomain {
    enabled: bool,
    previews_enabled: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubdomainPatch {
    enabled: bool,
    previews_enabled: Option<bool>,
}

pub(in crate::workers_http::v4) async fn get_subdomain(
    State(state): State<HttpState>,
    Path(path): Path<(String, String)>,
    request: Request,
) -> Response {
    settings_read_context(&state, &path, &request, V4Permission::Read).map_or_else(
        HttpError::into_response,
        |context| {
            success_response(
                context,
                Subdomain {
                    enabled: false,
                    previews_enabled: false,
                },
            )
        },
    )
}

pub(in crate::workers_http::v4) async fn post_subdomain(
    State(state): State<HttpState>,
    Path(path): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match settings_read_context(&state, &path, &request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let body = match json_body::<SubdomainPatch>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    if body.enabled || body.previews_enabled.unwrap_or(false) {
        return error_response(V4Error::Unsupported, context.request_id());
    }
    success_response(
        context,
        Subdomain {
            enabled: false,
            previews_enabled: false,
        },
    )
}

pub(in crate::workers_http::v4) async fn delete_subdomain(
    State(state): State<HttpState>,
    Path(path): Path<(String, String)>,
    request: Request,
) -> Response {
    settings_read_context(&state, &path, &request, V4Permission::ProductWrite).map_or_else(
        HttpError::into_response,
        |context| {
            success_response(
                context,
                Subdomain {
                    enabled: false,
                    previews_enabled: false,
                },
            )
        },
    )
}
