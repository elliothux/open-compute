use super::*;
use crate::run::daemon_control::InstanceView;
use open_compute_core::InstanceId;

#[test]
fn account_id_is_the_instance_id_without_a_hash_projection() {
    let instance_id = InstanceId::generate();
    let authority = V4InstanceContext::new(instance_id, 1_000);
    assert_eq!(authority.public_id(), instance_id.as_str());
}

#[tokio::test]
async fn account_collections_use_public_ids_and_sibling_result_info() {
    let (state, authority) = state();
    let response = app(state.clone())
        .oneshot(
            Request::builder()
                .uri("/accounts?page=1&per_page=20")
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json(response).await;
    assert_eq!(body["result"][0]["id"], authority.public_id());
    assert_eq!(body["result_info"]["total_count"], 1);
    assert!(body["result"][0]["id"].as_str().unwrap().len() == 32);

    let detail = app(state)
        .oneshot(
            Request::builder()
                .uri(format!("/accounts/{}", authority.public_id()))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    assert_eq!(json(detail).await["result"]["type"], "standard");
}

#[tokio::test]
async fn daemon_admin_discovery_lists_registered_named_and_unnamed_instances() {
    let named = InstanceId::generate().to_string();
    let unnamed = InstanceId::generate().to_string();
    let views = vec![
        InstanceView {
            instance_id: named.clone(),
            name: Some("dev".to_owned()),
            state: "running".to_owned(),
            error: None,
        },
        InstanceView {
            instance_id: unnamed.clone(),
            name: None,
            state: "stopped".to_owned(),
            error: None,
        },
    ];
    let response = accounts::shared_discovery(
        "/client/v4/accounts",
        Some("page=1&per_page=5"),
        views.clone(),
        V4Role::Admin,
    )
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json(response).await;
    assert_eq!(body["result_info"]["total_count"], 2);
    let result = body["result"].as_array().unwrap();
    assert!(
        result
            .iter()
            .any(|item| item["id"] == named && item["name"] == "dev")
    );
    assert!(
        result
            .iter()
            .any(|item| item["id"] == unnamed && item["name"] == unnamed)
    );

    let response = accounts::shared_discovery(
        "/client/v4/memberships",
        Some("name=dev"),
        views,
        V4Role::Admin,
    )
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json(response).await;
    assert_eq!(body["result_info"]["total_count"], 1);
    assert_eq!(body["result"][0]["account"]["id"], named);
    assert_eq!(body["result"][0]["id"], named);
}
