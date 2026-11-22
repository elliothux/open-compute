//! Public D1 query regression for schema migrations across a daemon restart.

#[allow(
    dead_code,
    reason = "this case uses a subset of the shared daemon fixture"
)]
#[path = "../workflow_support/platform_process.rs"]
mod platform_process;

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use open_compute_artifacts::MockS3;
use open_compute_core::{SystemClock, config::DataConfig};
use open_compute_storage::PlatformStorage;
use platform_process::Client;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_d1_schema_migrations_preserve_rows_across_daemon_restart() {
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let root = temp.path();
    let mock = MockS3::spawn("open-compute").await;
    let (public, admin) = platform_process::distinct_addresses();
    let data = root.join("data");
    PlatformStorage::bootstrap(
        &DataConfig {
            path: data.clone(),
            master_key_file: data.join("keys/master.key"),
            ..DataConfig::default()
        },
        &SystemClock,
    )
    .unwrap();
    let config = platform_process::config(root, &data, &mock.endpoint, public, admin);
    let log = root.join("ocd.log");
    // Release HTTP connections before each graceful daemon shutdown.
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .pool_max_idle_per_host(0)
        .build_http();
    let mut process = platform_process::spawn(&config, &log);
    platform_process::ready(&client, admin, &mut process).await;

    let accounts = api(&client, admin, "/client/v4/accounts", None, true).await;
    let account = accounts[0]["id"].as_str().unwrap();
    let collection = format!("/client/v4/accounts/{account}/d1/database");
    let database = api(
        &client,
        admin,
        &collection,
        Some(json!({"name": "schema-migrations"})),
        true,
    )
    .await;
    let id = database["uuid"].as_str().unwrap();
    let database_path = format!("{collection}/{id}");
    let query_path = format!("{database_path}/query");
    let query = |body, success| api(&client, admin, &query_path, Some(body), success);
    let sql = |sql| query(json!({"sql": sql}), true);

    for statement in [
        "CREATE TABLE entries(id INTEGER PRIMARY KEY, value TEXT, obsolete TEXT)",
        "INSERT INTO entries VALUES (1, 'alpha', 'old-a'), (7, 'beta', 'old-b')",
        "ALTER TABLE entries RENAME COLUMN value TO label",
        "ALTER TABLE entries DROP COLUMN obsolete",
        "ALTER TABLE entries RENAME TO items",
    ] {
        sql(statement).await;
    }
    let readback = "SELECT * FROM items ORDER BY id";
    let rows = json!([{"id": 1, "label": "alpha"}, {"id": 7, "label": "beta"}]);
    assert_eq!(sql(readback).await[0]["results"], rows);

    // Drizzle's copy-and-swap statements share one request and transaction.
    let rebuilt = sql(
        "CREATE TABLE replacement(id INTEGER PRIMARY KEY, label TEXT NOT NULL);
         INSERT INTO replacement(id, label) SELECT id, label FROM items;
         DROP TABLE items;
         ALTER TABLE replacement RENAME TO items",
    )
    .await;
    assert_eq!(rebuilt.as_array().unwrap().len(), 4);
    assert_eq!(sql(readback).await[0]["results"], rows);

    // The second statement also checks that ALTER's temporary schema access ends with that statement.
    query(
        json!({"batch": [
            {"sql": "ALTER TABLE items RENAME TO moved"},
            {"sql": "SELECT sql FROM temp.sqlite_schema"}
        ]}),
        false,
    )
    .await;
    assert_eq!(sql(readback).await[0]["results"], rows);
    assert_eq!(
        sql("PRAGMA table_info(moved)").await[0]["results"],
        json!([])
    );

    for denied in [
        "ALTER TABLE __open_compute_meta RENAME TO moved",
        "ALTER TABLE refinery_schema_history RENAME COLUMN name TO renamed",
        "UPDATE temp.sqlite_schema SET sql = NULL",
    ] {
        query(json!({"sql": denied}), false).await;
    }
    sql("ALTER TABLE items ADD COLUMN active INTEGER NOT NULL DEFAULT 1").await;

    process.stop().await;
    process.restart(&config, &log);
    platform_process::ready(&client, admin, &mut process).await;

    let reopened = api(&client, admin, &database_path, None, true).await;
    assert_eq!(reopened["uuid"], id);
    assert_eq!(
        sql(readback).await[0]["results"],
        json!([
            {"id": 1, "label": "alpha", "active": 1},
            {"id": 7, "label": "beta", "active": 1}
        ])
    );
    let schema = sql("PRAGMA table_info(items)").await;
    let columns: Vec<_> = schema[0]["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|column| {
            json!([
                column["name"],
                column["type"],
                column["pk"],
                column["notnull"]
            ])
        })
        .collect();
    assert_eq!(
        json!(columns),
        json!([
            ["id", "INTEGER", 1, 0],
            ["label", "TEXT", 0, 1],
            ["active", "INTEGER", 0, 1]
        ])
    );
    process.stop().await;
}

async fn api(
    client: &Client,
    address: SocketAddr,
    path: &str,
    body: Option<Value>,
    success: bool,
) -> Value {
    let method = if body.is_some() {
        Method::POST
    } else {
        Method::GET
    };
    let request = Request::builder()
        .method(&method)
        .uri(format!("http://{address}{path}"))
        .header(header::AUTHORIZATION, "Bearer workflow-deployer")
        .header(header::CONTENT_TYPE, "application/json")
        .body(
            body.as_ref()
                .map_or_else(Body::empty, |value| Body::from(value.to_string())),
        )
        .unwrap();
    let (status, bytes) = tokio::time::timeout(Duration::from_secs(10), async {
        let response = client.request(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(Body::new(response.into_body()), 65536)
            .await
            .unwrap();
        (status, bytes)
    })
    .await
    .unwrap();
    let envelope: Value = serde_json::from_slice(&bytes).unwrap();
    let context = format!("{method} {path} {body:?}: {status} {envelope}");
    assert_eq!(envelope["success"], success, "{context}");
    let errors = envelope["errors"].as_array().unwrap();
    if success {
        assert_eq!(status, StatusCode::OK, "{context}");
        assert!(errors.is_empty(), "{context}");
    } else {
        assert!(
            status.is_client_error() || status.is_server_error(),
            "{context}"
        );
        assert!(!errors.is_empty(), "{context}");
        assert!(envelope["result"].is_null(), "{context}");
    }
    envelope["result"].clone()
}
