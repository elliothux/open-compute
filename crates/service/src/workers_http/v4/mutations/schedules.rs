use super::*;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Schedule {
    cron: String,
    #[serde(skip_deserializing, skip_serializing_if = "Option::is_none")]
    created_on: Option<String>,
    #[serde(skip_deserializing, skip_serializing_if = "Option::is_none")]
    modified_on: Option<String>,
}

#[derive(Serialize)]
struct Schedules {
    schedules: Vec<Schedule>,
}

pub(in crate::workers_http::v4) async fn get_schedules(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let result = active_snapshot(&state, &account, &script).and_then(|(_, snapshot)| {
        CronRepository::new(worker_api(&state)?.storage.db())
            .version_config(snapshot.version.id)
            .map_err(|error| V4Error::from(&error))?
            .declarations
            .into_iter()
            .map(|declaration| {
                let time = crate::cloudflare_v4::iso_timestamp(declaration.created_at_ms)?;
                Ok(Schedule {
                    cron: declaration.expression,
                    created_on: Some(time.clone()),
                    modified_on: Some(time),
                })
            })
            .collect::<Result<Vec<_>, V4Error>>()
            .map(|schedules| Schedules { schedules })
    });
    respond(context, result)
}

pub(in crate::workers_http::v4) async fn put_schedules(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match authorize(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let schedules = match json_body::<Vec<Schedule>>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let crons = schedules.iter().map(|value| value.cron.clone()).collect();
    match mutate(
        &state,
        &account,
        &script,
        BTreeMap::new(),
        Some(crons),
        context.request_id(),
    )
    .await
    {
        Ok(()) => success_response(context, Schedules { schedules }),
        Err(error) => platform_error(context.request_id(), &error),
    }
}
