//! One serial ordinary-daemon case; no SDK reconstruction or private authority writes.

use super::python_support::capture::Capture;
use super::python_support::fixture::{Fixture, RequestTarget};
use super::python_support::{PYTHON_SECRETS, platform_process};
use super::{PEER, PYTHON};
use super::{assertions, websockets};
use serde_json::{Value, json};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p21_python_durable_objects_fetch_rpc_storage_alarm_restart_rollback() {
    let capture = Capture::load(
        "test/fixtures/python-durable-objects/active",
        "test/applications/python-durable-objects/src",
        &[],
    )
    .await;
    let retired = Capture::load(
        "test/fixtures/python-durable-objects/retired",
        "test/applications/python-durable-objects/retired",
        &[],
    )
    .await;
    let mut fixture = Fixture::new(None).await;
    let first = python_version(&fixture, &capture, "first", PYTHON_SECRETS[0], true).await;
    let first_record = fixture.record(PYTHON, &first);
    fixture.promote(PYTHON, &first).await;
    let peer = fixture.upload_javascript(
        PEER, include_str!("peer.js"), &[
            namespace("LOCAL", PEER),
            json!({"name":"REVISION","type":"plain_text","text":"javascript"}),
            json!({"name":"TOKEN","type":"secret_text","text":PYTHON_SECRETS[0]}),
        ],
        Some(json!({"default":{"type":"worker"},"Counter":{"type":"durable-object","storage":"sqlite"}})),
        None,
    ).await;
    fixture.promote(PEER, &peer).await;

    let python_ids = fixture.invoke(PYTHON, "/ids").await;
    let peer_ids = fixture.invoke(PEER, "/ids").await;
    for ids in [&python_ids, &peer_ids] {
        assert_eq!(ids["named"], ids["again"]);
        assert_eq!(ids["named"], ids["parsed"]);
        assert_ne!(ids["named"], ids["unique"]);
        assert_eq!(ids["unique"].as_str().unwrap().len(), 64);
    }
    assert_ne!(python_ids["unique"], peer_ids["unique"]);
    assert_ne!(python_ids["named"], peer_ids["named"]);
    fixture.invoke(PYTHON, "/increment").await;
    assertions::state(&fixture, PYTHON, 1, "first").await;
    fixture.invoke(PYTHON, "/increment").await;
    let before = assertions::state(&fixture, PYTHON, 2, "first").await;
    assert_eq!(before["id"], python_ids["named"]);
    fixture.invoke(PEER, "/increment").await;
    fixture.invoke(PEER, "/increment").await;
    let local_before = assertions::state(&fixture, PEER, 2, "javascript").await;
    assertions::values(&fixture, 2, "first").await;
    websockets::verify(&fixture, 1).await;

    // Abort replaces the live actor while preserving committed SQLite/KV state.
    let (status, _, _) = fixture
        .request(
            "/replace",
            "GET",
            "application/json",
            Vec::new(),
            RequestTarget::Worker(PYTHON),
        )
        .await;
    assert_eq!(status, 500);
    let replaced = assertions::state(&fixture, PYTHON, 2, "first").await;
    assert_eq!(replaced["id"], before["id"]);
    assert!(replaced["boot"].as_u64().unwrap() > before["boot"].as_u64().unwrap());

    let due = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
        + 30_000;
    for (caller, ids) in [(PYTHON, &python_ids), (PEER, &peer_ids)] {
        assert_eq!(
            fixture.invoke(caller, &format!("/alarm?at={due}")).await,
            json!({"armed":due})
        );
        assertions::scheduled(&fixture, ids["named"].as_str().unwrap(), due);
    }
    fixture.process.stop().await;
    fixture.ciphertext(&first_record).await;
    fixture.mock.clear_recorded();
    fixture.process.restart(&fixture.config, &fixture.log);
    platform_process::ready(&fixture.client, fixture.admin, &mut fixture.process).await;
    let restored = assertions::state(&fixture, PYTHON, 2, "first").await;
    let local_restored = assertions::state(&fixture, PEER, 2, "javascript").await;
    assert_eq!(restored["id"], before["id"]);
    assert!(restored["boot"].as_u64().unwrap() > replaced["boot"].as_u64().unwrap());
    assert!(local_restored["boot"].as_u64().unwrap() > local_before["boot"].as_u64().unwrap());
    assertions::alarms(&fixture).await;
    assertions::values(&fixture, 2, "first").await;
    websockets::counters(&fixture, 1).await;
    websockets::verify(&fixture, 2).await;
    assert_eq!(fixture.record(PYTHON, &first), first_record);
    assertions::no_prepare_upload(&fixture);

    let second = python_version(&fixture, &capture, "second", PYTHON_SECRETS[1], true).await;
    let second_record = fixture.record(PYTHON, &second);
    assert_ne!(
        first_record.prepared_identity_sha256,
        second_record.prepared_identity_sha256
    );
    assert_ne!(first_record.artifact_sha256, second_record.artifact_sha256);
    assert_eq!(fixture.active(PYTHON), normalized(&first));
    assertions::state(&fixture, PYTHON, 2, "first").await;
    fixture.promote(PYTHON, &second).await;
    assertions::state(&fixture, PYTHON, 2, "second").await;
    assertions::values(&fixture, 2, "second").await;
    fixture.invoke(PYTHON, "/increment").await;
    assertions::state(&fixture, PYTHON, 3, "second").await;
    fixture.promote(PYTHON, &first).await;
    assertions::state(&fixture, PYTHON, 3, "first").await;
    assertions::values(&fixture, 3, "first").await;
    assert_eq!(fixture.record(PYTHON, &first), first_record);
    assert_eq!(fixture.record(PYTHON, &second), second_record);

    // Each language owns its namespace. Local retirement changes Python
    // authority while the independent JavaScript actor remains available.
    let retired_version =
        python_version(&fixture, &retired, "retired", PYTHON_SECRETS[1], false).await;
    let retired_record = fixture.record(PYTHON, &retired_version);
    fixture.promote(PYTHON, &retired_version).await;
    assertions::retired(&fixture).await;
    assert_eq!(fixture.record(PYTHON, &retired_version), retired_record);

    fixture.process.stop().await;
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
                .any(|part| part == secret.as_bytes())
        );
    }
    println!(
        "python-durable-objects-evidence: {}",
        json!({
            "activeUploadSha256":capture.sha256,"retiredUploadSha256":retired.sha256,
            "firstVersion":first,"secondVersion":second,"retiredVersion":retired_version,
            "javascriptVersion":peer,"objectId":before["id"],"countAfterRollback":3,
            "firstPreparedIdentitySha256":hex::encode(first_record.prepared_identity_sha256),
            "secondPreparedIdentitySha256":hex::encode(second_record.prepared_identity_sha256),
            "pythonClassRetired":true,
            "selfOwnedNamespaces":true,
            "webSocketRounds":2,"textBinaryAndCleanClose":true,"alarmAndAttachmentReplay":true,
        })
    );
}

fn normalized(version: &str) -> String {
    uuid::Uuid::parse_str(version).unwrap().to_string()
}

fn namespace(name: &str, script: &str) -> Value {
    json!({"name":name,"type":"durable_object_namespace","class_name":"Counter","script_name":script})
}

async fn python_version(
    fixture: &Fixture,
    capture: &Capture,
    revision: &str,
    secret: &str,
    live: bool,
) -> String {
    let mut bindings = vec![
        json!({"name":"REVISION","type":"plain_text","text":revision}),
        json!({"name":"TOKEN","type":"secret_text","text":secret}),
    ];
    if live {
        bindings.push(namespace("OBJECTS", PYTHON));
    }
    let (status, _, bytes, version) = fixture
        .upload_version(
            PYTHON,
            &Capture::content_type(),
            capture.render(&bindings, None),
        )
        .await;
    let response: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        status, 200,
        "Python Durable Object upload/prepare failed for {revision}: {}",
        response["errors"]
    );
    assert_eq!(response["success"], true);
    version.unwrap().to_string()
}
