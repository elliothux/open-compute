//! Cloudflare v4 envelope and public-identifier assertions for the aggregate Gate.

use axum::Router;
use axum::http::StatusCode;
use serde_json::Value;

use super::admin_json;

pub(crate) struct ProductIds {
    pub(crate) account: String,
    pub(crate) kv: String,
    pub(crate) kv_other: String,
    pub(crate) d1: String,
}

pub(crate) async fn product_ids(router: &Router) -> ProductIds {
    let (status, accounts) =
        admin_json(router, "GET", "/client/v4/accounts", Value::Null, None).await;
    assert_envelope(status, &accounts);
    assert_eq!(status, StatusCode::OK, "{accounts}");
    let account = accounts["result"][0]["id"].as_str().unwrap().to_owned();
    assert_public_id(&account);

    let (status, kv) = admin_json(
        router,
        "GET",
        &format!("/client/v4/accounts/{account}/storage/kv/namespaces"),
        Value::Null,
        None,
    )
    .await;
    assert_envelope(status, &kv);
    assert_eq!(status, StatusCode::OK, "{kv}");
    let namespaces = kv["result"].as_array().unwrap();
    assert_eq!(namespaces.len(), 2);
    let kv = named_public_id(namespaces, "title", "id", "combined-kv");
    let kv_other = named_public_id(namespaces, "title", "id", "combined-kv-other");

    let (status, r2) = admin_json(
        router,
        "GET",
        &format!("/client/v4/accounts/{account}/r2/buckets"),
        Value::Null,
        None,
    )
    .await;
    assert_envelope(status, &r2);
    assert_eq!(status, StatusCode::OK, "{r2}");
    assert_eq!(r2["result"]["buckets"].as_array().unwrap().len(), 2);

    let (status, d1_catalog) = admin_json(
        router,
        "GET",
        &format!("/client/v4/accounts/{account}/d1/database"),
        Value::Null,
        None,
    )
    .await;
    assert_envelope(status, &d1_catalog);
    assert_eq!(status, StatusCode::OK, "{d1_catalog}");
    let databases = d1_catalog["result"].as_array().unwrap();
    assert_eq!(databases.len(), 3);
    let d1 = named_public_id(databases, "name", "uuid", "combined-d1");
    ProductIds {
        account,
        kv,
        kv_other,
        d1,
    }
}

fn named_public_id(entries: &[Value], name_key: &str, id_key: &str, name: &str) -> String {
    let id = entries
        .iter()
        .find(|entry| entry[name_key] == name)
        .and_then(|entry| entry[id_key].as_str())
        .unwrap()
        .to_owned();
    assert_public_id(&id);
    id
}

pub(crate) fn assert_public_id(value: &str) {
    assert_eq!(value.len(), 32);
    assert!(value.bytes().all(|byte| byte.is_ascii_hexdigit()));
}

pub(crate) fn assert_envelope(status: StatusCode, body: &Value) {
    assert_eq!(body["success"], status.is_success(), "{status}: {body}");
    assert!(body["errors"].is_array(), "{status}: {body}");
    assert!(body["messages"].is_array(), "{status}: {body}");
    if status.is_success() {
        assert!(!body["result"].is_null(), "{status}: {body}");
    } else {
        assert!(body["result"].is_null(), "{status}: {body}");
    }
}
