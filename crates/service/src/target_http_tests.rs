use super::*;
use open_compute_core::{TargetApiBaseUrl, TargetName};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
struct FixtureHttp(Mutex<HashMap<String, Vec<u8>>>);

impl FixtureHttp {
    fn insert(&self, url: String, value: &serde_json::Value) {
        self.0
            .lock()
            .unwrap()
            .insert(url, serde_json::to_vec(value).unwrap());
    }
}

impl TargetHttp for FixtureHttp {
    fn get<'a>(&'a self, url: &'a str, _token: &'a SecretString) -> GetFuture<'a> {
        Box::pin(async move {
            self.0
                .lock()
                .unwrap()
                .get(url)
                .cloned()
                .ok_or_else(|| target_unavailable("fixture target response is missing"))
        })
    }
}

fn record() -> TargetRecord {
    TargetRecord {
        schema_version: 1,
        name: "remote".parse::<TargetName>().unwrap(),
        api_base_url: "https://compute.example/client/v4"
            .parse::<TargetApiBaseUrl>()
            .unwrap(),
        account_id: "0123456789abcdef0123456789abcdef".parse().unwrap(),
        token_file: "/private/deployer.token".into(),
        created_at: 1,
    }
}

#[tokio::test]
async fn probe_requires_matching_account_and_valid_capabilities() {
    let http = FixtureHttp::default();
    let record = record();
    http.insert(
        record
            .api_base_url
            .endpoint(&format!("/accounts/{}", record.account_id)),
        &serde_json::json!({"success": true, "result": {"id": record.account_id}}),
    );
    http.insert(
        record.api_base_url.endpoint("/open-compute/capabilities"),
        &serde_json::json!({"success": true, "result": {"wrangler_version": "4.127.1"}}),
    );
    let result = probe_target(&http, &record, &SecretString::new("secret"))
        .await
        .unwrap();
    assert_eq!(result.wrangler_version, "4.127.1");
}

#[tokio::test]
async fn probe_rejects_mismatched_account_and_invalid_version() {
    let http = FixtureHttp::default();
    let record = record();
    let account_url = record
        .api_base_url
        .endpoint(&format!("/accounts/{}", record.account_id));
    http.insert(
        account_url.clone(),
        &serde_json::json!({"success": true, "result": {"id": "1123456789abcdef0123456789abcdef"}}),
    );
    assert!(
        probe_target(&http, &record, &SecretString::new("secret"))
            .await
            .is_err()
    );
    http.insert(
        account_url,
        &serde_json::json!({"success": true, "result": {"id": record.account_id}}),
    );
    http.insert(
        record.api_base_url.endpoint("/open-compute/capabilities"),
        &serde_json::json!({"success": true, "result": {"wrangler_version": "latest"}}),
    );
    assert!(
        probe_target(&http, &record, &SecretString::new("secret"))
            .await
            .is_err()
    );
}
