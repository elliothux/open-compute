//! One real-daemon Workflow case using the unchanged SDK and SQLite scheduler.

use super::assertions;
use super::python_support::capture::Capture;
use super::python_support::fixture::Fixture;
use super::python_support::{PYTHON_SECRETS, platform_process};
use super::{FLOW, PEER, PEER_FLOW, PYTHON};
use serde_json::{Value, json};
use std::fs;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p21_python_workflows_steps_events_retry_pause_restart_rollback() {
    let capture = Capture::load(
        "test/fixtures/python-workflows",
        "test/applications/python-workflows/src",
        &[],
    )
    .await;
    let mut fixture = Fixture::new(None).await;
    let kv = fixture
        .api(
            "/storage/kv/namespaces",
            "POST",
            Some(json!({"title":"python-workflows-effects"})),
        )
        .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let first = python_version(&fixture, &capture, &kv, "first", PYTHON_SECRETS[0]).await;
    let first_record = fixture.record(PYTHON, &first);
    fixture.promote(PYTHON, &first).await;
    // The current v4 adapter supports self-owned Workflow bindings. Each
    // language owns its Flow; both observe external effects in the same KV.
    let peer = fixture
        .upload_javascript(
            PEER,
            include_str!("peer.js"),
            &[
                json!({"name":"KV","type":"kv_namespace","namespace_id":kv}),
                workflow_binding(PEER, PEER_FLOW),
                json!({"name":"REVISION","type":"plain_text","text":"javascript"}),
                json!({"name":"TOKEN","type":"secret_text","text":PYTHON_SECRETS[0]}),
            ],
            Some(json!({"default":{"type":"worker"},"Flow":{"type":"workflow","name":PEER_FLOW}})),
            None,
        )
        .await;
    fixture.promote(PEER, &peer).await;
    assert_eq!(
        fixture.invoke(PYTHON, "/invalid").await,
        json!({"rejected":true})
    );
    for mode in ["normal", "retry", "fail", "caught"] {
        let id = format!("python-{mode}");
        assertions::create(&fixture, PYTHON, &id, mode).await;
        let value = assertions::status(
            &fixture,
            PYTHON,
            &id,
            if mode == "fail" {
                "errored"
            } else {
                "complete"
            },
        )
        .await;
        if mode == "fail" {
            assert!(!value["error"]["message"].as_str().unwrap().is_empty());
        } else {
            assert_eq!(
                value["output"],
                if mode == "caught" {
                    json!({"caught":true})
                } else {
                    assertions::output("first", None)
                }
            );
        }
        assertions::effects(
            &fixture,
            PEER,
            &id,
            if mode == "retry" { 2 } else { 1 },
            "first",
        )
        .await;
    }
    let batch = "python-batch";
    assert_eq!(
        fixture.invoke(PYTHON, &format!("/batch?id={batch}")).await,
        json!({"ids":[format!("{batch}-a"),format!("{batch}-b")]})
    );
    for suffix in ["a", "b"] {
        let id = format!("{batch}-{suffix}");
        assert_eq!(
            assertions::status(&fixture, PYTHON, &id, "complete").await["output"],
            assertions::output("first", None)
        );
        assertions::effects(&fixture, PEER, &id, 1, "first").await;
    }

    assertions::create(&fixture, PEER, "javascript-normal", "normal").await;
    assert_eq!(
        assertions::status(&fixture, PEER, "javascript-normal", "complete").await["output"],
        assertions::output("javascript", None)
    );
    assertions::effects(&fixture, PYTHON, "javascript-normal", 1, "javascript").await;

    assertions::create(&fixture, PYTHON, "python-wait", "wait").await;
    assertions::status(&fixture, PYTHON, "python-wait", "waiting").await;
    assert_eq!(
        fixture.invoke(PYTHON, "/pause?id=python-wait").await,
        json!({"paused":true})
    );
    assertions::status(&fixture, PYTHON, "python-wait", "paused").await;
    let python_step = assertions::prepare_record(&fixture, FLOW, "python-wait");
    assert_eq!(python_step.0, normalized(&first));
    assert_eq!(python_step.2, 1);
    fixture.process.stop().await;
    fixture.ciphertext(&first_record).await;
    fixture.mock.clear_recorded();
    fixture.process.restart(&fixture.config, &fixture.log);
    platform_process::ready(&fixture.client, fixture.admin, &mut fixture.process).await;
    assertions::status(&fixture, PYTHON, "python-wait", "paused").await;
    assert_eq!(
        assertions::prepare_record(&fixture, FLOW, "python-wait"),
        python_step
    );
    assertions::no_prepare_upload(&fixture);
    assert_eq!(fixture.record(PYTHON, &first), first_record);

    let second = python_version(&fixture, &capture, &kv, "second", PYTHON_SECRETS[1]).await;
    let second_record = fixture.record(PYTHON, &second);
    assert_ne!(
        first_record.prepared_identity_sha256,
        second_record.prepared_identity_sha256
    );
    assert_ne!(first_record.artifact_sha256, second_record.artifact_sha256);
    assert_eq!(fixture.active(PYTHON), normalized(&first));
    fixture.promote(PYTHON, &second).await;
    assert_eq!(
        fixture.invoke(PYTHON, "/event?id=python-wait").await,
        json!({"sent":true})
    );
    assert_eq!(
        fixture.invoke(PYTHON, "/resume?id=python-wait").await,
        json!({"resumed":true})
    );
    assert_eq!(
        assertions::status(&fixture, PYTHON, "python-wait", "complete").await["output"],
        assertions::output("first", Some(&json!({"unicode":"µ☁"})))
    );
    assertions::effects(&fixture, PEER, "python-wait", 1, "first").await;
    assert_eq!(
        assertions::prepare_record(&fixture, FLOW, "python-wait"),
        python_step
    );
    assertions::create(&fixture, PYTHON, "python-second", "normal").await;
    assert_eq!(
        assertions::status(&fixture, PYTHON, "python-second", "complete").await["output"],
        assertions::output("second", None)
    );
    fixture.promote(PYTHON, &first).await;
    fixture.mock.clear_recorded();
    fixture.restart().await;
    assert_eq!(
        assertions::status(&fixture, PYTHON, "python-second", "complete").await["output"],
        assertions::output("second", None)
    );
    assertions::effects(&fixture, PEER, "python-wait", 1, "first").await;
    assertions::no_prepare_upload(&fixture);
    // Workflow definition updates are separate from HTTP deployment promotion.
    assertions::create(&fixture, PYTHON, "python-before-retarget", "normal").await;
    assert_eq!(
        assertions::status(&fixture, PYTHON, "python-before-retarget", "complete").await["output"],
        assertions::output("second", None)
    );
    let retarget = fixture
        .api(
            &format!("/workflows/{FLOW}"),
            "PUT",
            Some(json!({"script_name":PYTHON,"class_name":"Flow"})),
        )
        .await;
    assert_eq!(retarget["script_name"], PYTHON);
    assert_eq!(retarget["class_name"], "Flow");
    assertions::create(&fixture, PYTHON, "python-rollback", "normal").await;
    assert_eq!(
        assertions::status(&fixture, PYTHON, "python-rollback", "complete").await["output"],
        assertions::output("first", None)
    );

    assertions::create(&fixture, PYTHON, "python-terminate", "wait").await;
    assertions::status(&fixture, PYTHON, "python-terminate", "waiting").await;
    assert_eq!(
        fixture
            .invoke(PYTHON, "/terminate?id=python-terminate")
            .await,
        json!({"terminated":true})
    );
    assertions::status(&fixture, PYTHON, "python-terminate", "terminated").await;
    assertions::effects(&fixture, PEER, "python-terminate", 1, "first").await;
    fixture.restart().await;
    assertions::status(&fixture, PYTHON, "python-terminate", "terminated").await;
    assert_eq!(fixture.record(PYTHON, &first), first_record);
    assert_eq!(fixture.record(PYTHON, &second), second_record);
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
        "python-workflows-evidence: {}",
        json!({
            "uploadSha256":capture.sha256,"pythonFirstVersion":first,"pythonSecondVersion":second,"javascriptVersion":peer,
            "firstPreparedIdentitySha256":hex::encode(first_record.prepared_identity_sha256),
            "secondPreparedIdentitySha256":hex::encode(second_record.prepared_identity_sha256),
            "committedPrepareStepSha256":python_step.3,"pausedRestartPreservesCompletedStep":true,
            "oldInstanceKeepsFirstVersionAfterPromotion":true,
            "rollbackDefinitionRetargetedThroughPublicApi":true,
        })
    );
}

fn normalized(version: &str) -> String {
    uuid::Uuid::parse_str(version).unwrap().to_string()
}

fn workflow_binding(script: &str, name: &str) -> Value {
    json!({"name":"FLOW","type":"workflow","workflow_name":name,"class_name":"Flow","script_name":script})
}

async fn python_version(
    fixture: &Fixture,
    capture: &Capture,
    kv: &str,
    revision: &str,
    secret: &str,
) -> String {
    let (status, _, bytes, version) = fixture
        .upload_version(
            PYTHON,
            &Capture::content_type(),
            capture.render(
                &[
                    json!({"name":"REVISION","type":"plain_text","text":revision}),
                    json!({"name":"TOKEN","type":"secret_text","text":secret}),
                    json!({"name":"KV","type":"kv_namespace","namespace_id":kv}),
                    workflow_binding(PYTHON, FLOW),
                ],
                None,
            ),
        )
        .await;
    assert_eq!(status, 200, "Python Workflow upload/prepare failed");
    let response: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(response["success"], true);
    version.unwrap().to_string()
}
