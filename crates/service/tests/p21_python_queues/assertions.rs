//! Public observations and read-only persistence checks for the Queue scenario.

use super::python_support::PYTHON_SECRETS;
use super::python_support::fixture::{Fixture, RequestTarget};
use super::{DLQ, EVENTS, PEER, PYTHON};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::time::{Duration, Instant};

pub(super) async fn create_queue(fixture: &Fixture, name: &str) -> String {
    fixture
        .api(
            "/queues",
            "POST",
            Some(json!({
                "queue_name":name,"settings":{"delivery_paused":true}
            })),
        )
        .await["queue_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

pub(super) async fn pause(fixture: &Fixture, queue: &str, paused: bool) {
    fixture
        .api(
            &format!("/queues/{queue}"),
            "PATCH",
            Some(json!({
                "settings":{"delivery_paused":paused}
            })),
        )
        .await;
}

pub(super) async fn set_consumer(
    fixture: &Fixture,
    queue: &str,
    consumer: Option<&str>,
    script: &str,
) -> String {
    let (path, method) = match consumer {
        Some(id) => (format!("/queues/{queue}/consumers/{id}"), "PUT"),
        None => (format!("/queues/{queue}/consumers"), "POST"),
    };
    fixture
        .api(
            &path,
            method,
            Some(json!({
                "type":"worker","script_name":script,"dead_letter_queue":DLQ,
                "settings":{"batch_size":1,"max_concurrency":1,"max_retries":1,
                            "max_wait_time_ms":0,"retry_delay":0},
            })),
        )
        .await["consumer_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

pub(super) async fn send_pair(fixture: &Fixture, phase: &str) {
    for (script, language) in [(PYTHON, "python"), (PEER, "javascript")] {
        assert_eq!(
            fixture
                .invoke(script, &format!("/send?phase={phase}-{language}"))
                .await,
            json!({"sent":4})
        );
    }
}

pub(super) async fn backlog(fixture: &Fixture, queue: &str) -> u64 {
    fixture
        .api(&format!("/queues/{queue}/metrics"), "GET", None)
        .await["backlog_count"]
        .as_u64()
        .unwrap()
}

pub(super) async fn wait_backlog(fixture: &Fixture, queue: &str, expected: u64) {
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let actual = backlog(fixture, queue).await;
        if actual == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Queue backlog did not become {expected}; last {actual}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

pub(super) async fn lookup(fixture: &Fixture, label: &str) -> Value {
    // The ordinary JS peer reads shared KV, so observing a Python consumer
    // cannot accidentally reuse a Python process-local memory variable.
    fixture
        .invoke(PEER, &format!("/lookup?label={label}"))
        .await
}

pub(super) async fn assert_no_observation(fixture: &Fixture, label: &str) {
    assert!(lookup(fixture, label).await.is_null());
}

pub(super) async fn wait_observation(fixture: &Fixture, label: &str, attempts: u64) -> Value {
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let result = lookup(fixture, label).await;
        if result["attempts"]
            .as_u64()
            .is_some_and(|count| count >= attempts)
        {
            return result;
        }
        assert!(
            Instant::now() < deadline,
            "missing consumer observation {label}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

pub(super) async fn verify_pair(fixture: &Fixture, queue: &str, phase: &str, revision: &str) {
    let mut ids = BTreeSet::new();
    for language in ["python", "javascript"] {
        for kind in ["json", "text", "bytes", "v8"] {
            let label = format!("{phase}-{language}-{kind}");
            let value = wait_observation(fixture, &label, 1).await;
            assert_eq!(value["queue"], EVENTS);
            assert_eq!(value["revision"], revision);
            assert_eq!(value["kind"], kind);
            assert_eq!(value["attempts"], 1);
            assert!(value["timestamp"].as_f64().unwrap() > 0.0);
            let id = value["id"].as_str().unwrap().to_owned();
            assert!(!id.is_empty());
            assert!(ids.insert(id));
            let expected = match kind {
                "json" => json!([null, true, 42, 1.25, "µ☁"]),
                "text" => json!(label),
                "bytes" => json!(label.as_bytes()),
                "v8" => {
                    assert_eq!(value["value"]["when"].as_f64(), Some(1_700_000_000.0));
                    assert_eq!(
                        value["value"]["values"],
                        json!([null, true, 42, 1.25, "µ☁"])
                    );
                    assert_eq!(value["value"]["unicode"], "你".repeat(50_000));
                    value["value"].clone()
                }
                _ => unreachable!(),
            };
            assert_eq!(value["value"], expected);
            assert_eq!(
                fixture
                    .invoke(PYTHON, &format!("/lookup?label={label}"))
                    .await,
                value
            );
        }
    }
    assert_eq!(ids.len(), 8);
    wait_backlog(fixture, queue, 0).await;
}

pub(super) fn assert_persisted_frames(fixture: &Fixture, expected: usize) {
    let connection = fixture.scheduler_connection();
    let mut statement = connection
        .prepare("SELECT content_type,body,state,attempts FROM queue_messages ORDER BY seq")
        .unwrap();
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, u64>(3)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(rows.len(), expected);
    for (index, (kind, body, state, attempts)) in rows.iter().enumerate() {
        assert_eq!(kind, ["json", "text", "bytes", "v8"][index % 4]);
        if kind == "v8" {
            assert!(body.starts_with(&[0xff, 15]));
            assert!((100_000..=128_000).contains(&body.len()));
        }
        assert!(!body.is_empty());
        assert_eq!(state, "ready");
        assert_eq!(*attempts, 0);
    }
}

pub(super) fn assert_dlq_body(fixture: &Fixture) {
    let id: String = fixture
        .connection()
        .query_row(
            "SELECT id FROM queues WHERE name=?1 AND state='ready'",
            [DLQ],
            |row| row.get(0),
        )
        .unwrap();
    let (kind, body): (String, Vec<u8>) = fixture
        .scheduler_connection()
        .query_row(
            "SELECT content_type,body FROM queue_messages WHERE queue_id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(kind, "json");
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"label":"python-dlq","action":"fail"})
    );
}

pub(super) fn assert_no_prepare_upload(fixture: &Fixture) {
    assert!(
        !fixture
            .mock
            .recorded()
            .iter()
            .any(|request| { request.method == "PUT" && request.path.contains("/artifacts/") }),
        "restore must not persist a newly prepared artifact"
    );
}

pub(super) async fn assert_referenced_deletion_rejected(fixture: &Fixture, queue: &str) {
    let (status, _, bytes) = fixture
        .request(
            &format!(
                "/client/v4/accounts/{}/queues/{queue}",
                fixture.public_account
            ),
            "DELETE",
            "",
            Vec::new(),
            RequestTarget::Admin,
        )
        .await;
    assert_eq!(status, 409, "referenced Queue deletion must be rejected");
    let response: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(response["success"], false);
    assert!(response["result"].is_null());
    assert_eq!(response["errors"][0]["code"], 9_100_006);
}

pub(super) async fn assert_clean_shutdown(fixture: &Fixture) {
    for lease in [
        "child.lease",
        "python-prepare.lease",
        "python-compile.lease",
    ] {
        assert!(!fixture.data.join("runtime").join(lease).exists());
    }
    for address in [fixture.public, fixture.admin] {
        assert!(tokio::net::TcpListener::bind(address).await.is_ok());
    }
    let log = fs::read(&fixture.log).unwrap();
    for secret in PYTHON_SECRETS {
        assert!(
            !log.windows(secret.len())
                .any(|bytes| bytes == secret.as_bytes()),
            "Queue handler failure exposed a Python secret in daemon logs"
        );
    }
}
