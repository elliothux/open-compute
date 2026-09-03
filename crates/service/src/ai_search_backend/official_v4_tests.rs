//! Official Cloudflare AI Search management route coverage.

use super::*;

fn official_ai_state(fixture: &SearchBehaviorFixture) -> (HttpState, String) {
    let storage = fixture._runtime.storage.clone();
    let identity = storage.identity();
    let account = AccountAuthority::new(
        identity.platform_id,
        identity.default_account_id,
        identity.created_at_ms,
    );
    let public_account = account.public_id().to_owned();
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        metrics,
        false,
        Some(SecretString::new("admin-token")),
    )
    .with_v4_tokens(
        SecretString::new("deployer-token"),
        SecretString::new("read-token"),
    )
    .with_cloudflare_v4_account(account)
    .with_search_api(
        SearchApiState::new(storage, fixture.pins.clone(), 5_000, Duration::from_secs(1))
            .with_ai_search(fixture.service.clone()),
    );
    (state, public_account)
}

async fn official_ai_send(
    state: &HttpState,
    method: &str,
    path: &str,
    token: &str,
    content_type: Option<&str>,
    body: impl Into<Body>,
) -> Response {
    let mut request = HttpRequest::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    if let Some(content_type) = content_type {
        request = request.header(header::CONTENT_TYPE, content_type);
    }
    v4_router(state.clone(), storage_router())
        .with_state(state.clone())
        .oneshot(request.body(body.into()).unwrap())
        .await
        .unwrap()
}

async fn official_ai_json(response: Response) -> Value {
    assert!(response.headers().contains_key(REQUEST_ID_HEADER));
    serde_json::from_slice(
        &to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn official_ai_search_routes_cover_the_frozen_29_operation_surface() {
    let fixture = SearchBehaviorFixture::create().await;
    let (state, account) = official_ai_state(&fixture);
    let namespaces = format!("/accounts/{account}/ai-search/namespaces");
    let main = format!("{namespaces}/search-behavior");

    let list = official_ai_send(
        &state,
        "GET",
        &namespaces,
        "read-token",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    assert_eq!(
        official_ai_json(list).await["result"][0]["name"],
        "search-behavior"
    );
    let wrong_account = official_ai_send(
        &state,
        "GET",
        "/accounts/00000000000000000000000000000000/ai-search/namespaces",
        "read-token",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(wrong_account.status(), StatusCode::NOT_FOUND);
    let _ = official_ai_json(wrong_account).await;
    let duplicate_query = official_ai_send(
        &state,
        "GET",
        &format!("{namespaces}?page=1&page=2"),
        "read-token",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(duplicate_query.status(), StatusCode::BAD_REQUEST);
    let _ = official_ai_json(duplicate_query).await;

    let bad_content_type = official_ai_send(
        &state,
        "POST",
        &namespaces,
        "deployer-token",
        Some("application/json;charset=utf-8;charset=utf-8"),
        json!({"name":"invalid"}).to_string(),
    )
    .await;
    assert_eq!(bad_content_type.status(), StatusCode::BAD_REQUEST);
    let duplicate_json = official_ai_send(
        &state,
        "POST",
        &namespaces,
        "deployer-token",
        Some("application/json"),
        r#"{"name":"first","name":"second"}"#,
    )
    .await;
    assert_eq!(duplicate_json.status(), StatusCode::BAD_REQUEST);
    let _ = official_ai_json(duplicate_json).await;
    let forbidden = official_ai_send(
        &state,
        "POST",
        &namespaces,
        "read-token",
        Some("application/json"),
        json!({"name":"forbidden"}).to_string(),
    )
    .await;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    let _ = official_ai_json(forbidden).await;

    let create_namespace = official_ai_send(
        &state,
        "POST",
        &namespaces,
        "deployer-token",
        Some("application/json"),
        json!({"name":"second","description":"managed"}).to_string(),
    )
    .await;
    assert_eq!(create_namespace.status(), StatusCode::OK);
    let second = format!("{namespaces}/second");
    let get_namespace =
        official_ai_send(&state, "GET", &second, "read-token", None, Body::empty()).await;
    assert_eq!(
        official_ai_json(get_namespace).await["result"]["description"],
        "managed"
    );
    let update_namespace = official_ai_send(
        &state,
        "PUT",
        &second,
        "deployer-token",
        Some("application/json"),
        json!({"description":"updated"}).to_string(),
    )
    .await;
    assert_eq!(update_namespace.status(), StatusCode::OK);
    let delete_namespace = official_ai_send(
        &state,
        "DELETE",
        &second,
        "deployer-token",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(delete_namespace.status(), StatusCode::OK);

    let instances = format!("{main}/instances");
    let instance_config = |id: &str| {
        json!({
            "id": id,
            "embedding_model": "@cf/qwen/qwen3-embedding-0.6b",
            "index_method": {"vector": false, "keyword": true},
            "indexing_options": {"keyword_tokenizer": "porter"},
            "retrieval_options": {"keyword_match_mode": "and"},
            "chunk_size": 32,
            "chunk_overlap": 0,
            "score_threshold": 0.0,
            "max_num_results": 10,
            "custom_metadata": []
        })
    };
    for id in ["docs", "disposable"] {
        let create = official_ai_send(
            &state,
            "POST",
            &instances,
            "deployer-token",
            Some("application/json"),
            instance_config(id).to_string(),
        )
        .await;
        assert_eq!(create.status(), StatusCode::OK);
    }
    let unsupported_connector = official_ai_send(
        &state,
        "POST",
        &instances,
        "deployer-token",
        Some("application/json"),
        json!({"id":"connector","cache":false}).to_string(),
    )
    .await;
    assert_eq!(unsupported_connector.status(), StatusCode::NOT_IMPLEMENTED);
    let list_instances = official_ai_send(
        &state,
        "GET",
        &format!("{instances}?per_page=1"),
        "read-token",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(list_instances.status(), StatusCode::OK);
    assert_eq!(
        official_ai_json(list_instances).await["result_info"]["per_page"],
        1
    );
    let docs = format!("{instances}/docs");
    let get_instance =
        official_ai_send(&state, "GET", &docs, "read-token", None, Body::empty()).await;
    let instance = official_ai_json(get_instance).await;
    assert_eq!(instance["result"]["id"], "docs");
    assert_eq!(instance["result"]["namespace"], "search-behavior");
    let update_instance = official_ai_send(
        &state,
        "PUT",
        &docs,
        "deployer-token",
        Some("application/json"),
        json!({"metadata":{"owner":"test"}}).to_string(),
    )
    .await;
    assert_eq!(update_instance.status(), StatusCode::OK);
    let stats = official_ai_send(
        &state,
        "GET",
        &format!("{docs}/stats"),
        "read-token",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(stats.status(), StatusCode::OK);
    let stats = official_ai_json(stats).await;
    assert_eq!(stats["result"]["degraded"], false);
    assert!(stats["result"]["engine"]["r2"]["objectCount"].is_number());
    assert!(stats["result"]["engine"]["vectorize"]["dimensions"].is_number());

    let search_payload = json!({
        "query": "alpha",
        "ai_search_options": {"retrieval": {"retrieval_type": "keyword"}}
    });
    let instance_search = official_ai_send(
        &state,
        "POST",
        &format!("{docs}/search"),
        "read-token",
        Some("application/json"),
        search_payload.to_string(),
    )
    .await;
    assert_eq!(instance_search.status(), StatusCode::OK);
    let namespace_search = official_ai_send(
        &state,
        "POST",
        &format!("{main}/search"),
        "read-token",
        Some("application/json"),
        json!({
            "query":"alpha",
            "ai_search_options": {
                "instance_ids":["docs"],
                "retrieval":{"retrieval_type":"keyword"}
            }
        })
        .to_string(),
    )
    .await;
    assert_eq!(namespace_search.status(), StatusCode::OK);
    for (chat_path, multi) in [
        (format!("{main}/chat/completions"), true),
        (format!("{docs}/chat/completions"), false),
    ] {
        let options = if multi {
            json!({
                "instance_ids":["docs"],
                "retrieval":{"retrieval_type":"keyword"}
            })
        } else {
            json!({"retrieval":{"retrieval_type":"keyword"}})
        };
        let chat = official_ai_send(
            &state,
            "POST",
            &chat_path,
            "read-token",
            Some("application/json"),
            json!({
                "messages":[{"role":"user","content":"alpha"}],
                "stream":false,
                "ai_search_options": options
            })
            .to_string(),
        )
        .await;
        assert_eq!(chat.status(), StatusCode::NOT_IMPLEMENTED);
    }

    let jobs = format!("{docs}/jobs");
    let create_job = official_ai_send(
        &state,
        "POST",
        &jobs,
        "deployer-token",
        Some("application/json"),
        json!({"description":"reindex"}).to_string(),
    )
    .await;
    assert_eq!(create_job.status(), StatusCode::OK);
    let job = official_ai_json(create_job).await;
    let job_id = job["result"]["id"].as_str().unwrap().to_owned();
    let list_jobs = official_ai_send(
        &state,
        "GET",
        &format!("{jobs}?per_page=0"),
        "read-token",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(list_jobs.status(), StatusCode::OK);
    let job_path = format!("{jobs}/{job_id}");
    let get_job =
        official_ai_send(&state, "GET", &job_path, "read-token", None, Body::empty()).await;
    assert_eq!(get_job.status(), StatusCode::OK);
    let cancel_job = official_ai_send(
        &state,
        "PATCH",
        &job_path,
        "deployer-token",
        Some("application/json"),
        json!({"action":"cancel"}).to_string(),
    )
    .await;
    assert_eq!(cancel_job.status(), StatusCode::OK);
    let job_logs = official_ai_send(
        &state,
        "GET",
        &format!("{job_path}/logs?per_page=0"),
        "read-token",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(job_logs.status(), StatusCode::OK);

    let items = format!("{docs}/items");
    let boundary = "official-ai-search-boundary";
    let unknown_part = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"bad.txt\"\r\n\r\nbad\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"unknown\"\r\n\r\nvalue\r\n--{boundary}--\r\n"
    );
    let rejected_upload = official_ai_send(
        &state,
        "POST",
        &items,
        "deployer-token",
        Some(&format!("multipart/form-data; boundary={boundary}")),
        unknown_part,
    )
    .await;
    assert_eq!(rejected_upload.status(), StatusCode::BAD_REQUEST);
    let _ = official_ai_json(rejected_upload).await;
    let multipart = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"guide.txt\"\r\nContent-Type: text/plain\r\n\r\nalpha beta\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{{}}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"wait_for_completion\"\r\n\r\nfalse\r\n--{boundary}--\r\n"
    );
    let upload = official_ai_send(
        &state,
        "POST",
        &items,
        "deployer-token",
        Some(&format!("multipart/form-data; boundary={boundary}")),
        multipart,
    )
    .await;
    assert_eq!(upload.status(), StatusCode::OK);
    let upload = official_ai_json(upload).await;
    let item_id = upload["result"]["id"].as_str().unwrap().to_owned();
    assert_eq!(upload["result"]["namespace"], "search-behavior");

    let list_items = official_ai_send(
        &state,
        "GET",
        &format!("{items}?per_page=0"),
        "read-token",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(list_items.status(), StatusCode::OK);
    let item = format!("{items}/{item_id}");
    let get_item = official_ai_send(&state, "GET", &item, "read-token", None, Body::empty()).await;
    assert_eq!(get_item.status(), StatusCode::OK);
    let download = official_ai_send(
        &state,
        "GET",
        &format!("{item}/download"),
        "read-token",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(download.status(), StatusCode::OK);
    assert!(download.headers().contains_key(REQUEST_ID_HEADER));
    assert_eq!(
        to_bytes(download.into_body(), 1024).await.unwrap(),
        "alpha beta"
    );
    let item_logs = official_ai_send(
        &state,
        "GET",
        &format!("{item}/logs?limit=1"),
        "read-token",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(item_logs.status(), StatusCode::OK);
    let item_logs = official_ai_json(item_logs).await;
    let cursor = item_logs["result_info"]["cursor"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(cursor.len() > 32 && !cursor.contains(item_id.as_str()));
    let next_logs = official_ai_send(
        &state,
        "GET",
        &format!("{item}/logs?limit=1&cursor={cursor}"),
        "read-token",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(next_logs.status(), StatusCode::OK);
    let chunks = official_ai_send(
        &state,
        "GET",
        &format!("{item}/chunks"),
        "read-token",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(chunks.status(), StatusCode::OK);
    let forged_cursor = official_ai_send(
        &state,
        "GET",
        &format!("{item}/logs?limit=50&cursor=forged"),
        "read-token",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(forged_cursor.status(), StatusCode::BAD_REQUEST);
    let _ = official_ai_json(forged_cursor).await;
    let put_item = official_ai_send(
        &state,
        "PUT",
        &items,
        "deployer-token",
        Some("application/json"),
        json!({"key":"guide.txt","next_action":"INDEX"}).to_string(),
    )
    .await;
    assert_eq!(put_item.status(), StatusCode::OK);
    let sync_item = official_ai_send(
        &state,
        "PATCH",
        &item,
        "deployer-token",
        Some("application/json"),
        json!({"next_action":"INDEX"}).to_string(),
    )
    .await;
    assert_eq!(sync_item.status(), StatusCode::OK);
    let delete_item = official_ai_send(
        &state,
        "DELETE",
        &item,
        "deployer-token",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(delete_item.status(), StatusCode::OK);

    let delete_instance = official_ai_send(
        &state,
        "DELETE",
        &format!("{instances}/disposable"),
        "deployer-token",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(delete_instance.status(), StatusCode::OK);
}
