use super::*;

pub(super) async fn exercise_d1_catalog(
    app: &Router,
    public_account: &str,
    source_prefix: &str,
) -> String {
    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/client/v4/accounts/{public_account}/d1/database?page=1&per_page=1&name=transfer"
                ))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    assert_eq!(response_json(listed).await["result_info"]["count"], 1);
    let fetched = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("{source_prefix}?fields=uuid,name,read_replication"))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fetched.status(), StatusCode::OK);
    assert_eq!(
        response_json(fetched).await["result"]["name"],
        "transfer-source-http"
    );
    let updated = app
        .clone()
        .oneshot(transfer_request(
            Method::PATCH,
            source_prefix,
            Body::from(r#"{"read_replication":{"mode":"disabled"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    for body in [
        r#"{}"#,
        r#"{"read_replication":{"mode":"invalid"}}"#,
        r#"{"read_replication":{"mode":"auto"}}"#,
    ] {
        let response = app
            .clone()
            .oneshot(transfer_request(
                Method::PUT,
                source_prefix,
                Body::from(body),
            ))
            .await
            .unwrap();
        assert!(!response.status().is_success());
    }
    for (suffix, body) in [
        ("query", r#"{"sql":"SELECT ? AS value","params":["text"]}"#),
        (
            "raw",
            r#"{"batch":[{"sql":"SELECT 1 AS integer_value, 1.5 AS real_value, NULL AS null_value, X'0102' AS blob_value"}]}"#,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(transfer_request(
                Method::POST,
                &format!("{source_prefix}/{suffix}"),
                Body::from(body),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "D1 {suffix}");
        assert_eq!(response_json(response).await["success"], true);
    }
    let created_database = app
        .clone()
        .oneshot(transfer_request(
            Method::POST,
            &format!("/client/v4/accounts/{public_account}/d1/database"),
            Body::from(r#"{"name":"created-via-http","read_replication":{"mode":"disabled"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(created_database.status(), StatusCode::OK);

    response_json(created_database).await["result"]["uuid"]
        .as_str()
        .unwrap()
        .to_owned()
}
