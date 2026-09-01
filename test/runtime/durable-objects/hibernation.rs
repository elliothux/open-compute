//! Hibernatable WebSocket and full storage-member assertions for the P0.7 matrix.

use super::*;

pub(super) fn storage_members(storage_result: &serde_json::Value) {
    for key in [
        "syncKv",
        "asyncKv",
        "asyncTransactionRollback",
        "transactionRollback",
        "transactionSync",
        "listOptions",
        "sqlCursor",
        "sync",
        "bookmarks",
        "pitrUnsupported",
        "alarms",
        "exports",
        "privateExportsHidden",
        "props",
        "facets",
        "containerAbsent",
        "id",
        "deleteAll",
        "blockConcurrency",
        "waitUntil",
    ] {
        assert_eq!(storage_result[key], true, "{key}: {storage_result}");
    }
}

pub(super) async fn facets(
    transport: &WorkerdTransport,
    account: AccountId,
    worker: WorkerId,
    deployment: &DeploymentRecord,
    generation: u64,
) {
    let response = dispatch(
        transport,
        account,
        worker,
        deployment,
        generation,
        "/facets?name=facet-owner",
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body);
    let result: serde_json::Value = serde_json::from_str(&response.body).unwrap();
    assert_eq!(result["first"], 1, "{result}");
    assert_eq!(result["second"], 2, "{result}");
    assert_eq!(result["props"]["marker"], "facet", "{result}");
    assert_eq!(result["id"], "facet-id", "{result}");
    assert_eq!(result["cloned"], 3, "{result}");
    assert_eq!(result["fresh"], 1, "{result}");
    assert_eq!(result["aborted"], true, "{result}");
    assert_eq!(result["afterAbort"], 3, "{result}");
}

pub(super) async fn check(
    transport: &WorkerdTransport,
    supervisor: &WorkerdSupervisor,
    account: AccountId,
    worker: WorkerId,
    deployment: &DeploymentRecord,
    generation: u64,
) {
    run_native_eviction_probe().await;

    let hibernate = dispatch(
        transport,
        account,
        worker,
        deployment,
        generation,
        "/hibernate?name=hibernate",
    )
    .await;
    assert_eq!(hibernate.status, 200, "{}", hibernate.body);
    let report: serde_json::Value = serde_json::from_str(&hibernate.body).unwrap();
    for key in [
        "auto",
        "echoed",
        "sockets",
        "tags",
        "attachment",
        "autoRequest",
        "autoResponse",
        "closed",
    ] {
        assert_eq!(report[key], true, "{key}: {report}");
    }

    let inspect = dispatch(
        transport,
        account,
        worker,
        deployment,
        generation,
        "/hibernate-inspect?name=hibernate",
    )
    .await;
    assert_eq!(inspect.status, 200, "{}", inspect.body);
    let after_close: serde_json::Value = serde_json::from_str(&inspect.body).unwrap();
    assert_eq!(after_close["sockets"], 0, "{after_close}");

    let held = dispatch(
        transport,
        account,
        worker,
        deployment,
        generation,
        "/hibernate-open?name=hibernate-hold",
    )
    .await;
    assert_eq!(held.status, 200, "{}", held.body);
    let held_report: serde_json::Value = serde_json::from_str(&held.body).unwrap();
    assert_eq!(held_report["sockets"], 1, "{held_report}");

    let abort = dispatch(
        transport,
        account,
        worker,
        deployment,
        generation,
        "/abort?name=abort-probe",
    )
    .await;
    assert_eq!(abort.status, 200, "{}", abort.body);
    let aborted: serde_json::Value = serde_json::from_str(&abort.body).unwrap();
    assert_eq!(aborted["aborted"], true, "{aborted}");
    assert_eq!(aborted["recovered"], true, "{aborted}");

    let old_pid = supervisor.snapshot().pid.unwrap();
    supervisor.report_unhealthy();
    wait_pid_change(supervisor, old_pid, Duration::from_secs(30)).await;
    let cleaned = dispatch(
        transport,
        account,
        worker,
        deployment,
        generation,
        "/hibernate-inspect?name=hibernate-hold",
    )
    .await;
    assert_eq!(cleaned.status, 200, "{}", cleaned.body);
    let cleaned_report: serde_json::Value = serde_json::from_str(&cleaned.body).unwrap();
    assert_eq!(
        cleaned_report["sockets"], 0,
        "process restart must drop in-memory sockets rather than preserve hibernation continuity: {cleaned_report}"
    );
}

async fn run_native_eviction_probe() {
    let binary = PathBuf::from(
        std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
            .expect("OPEN_COMPUTE_TEST_WORKERD must name the verified stock runtime"),
    );
    let storage = tempfile::tempdir().unwrap();
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new(binary)
            .arg("test")
            .arg("--experimental")
            .arg(format!(
                "--directory-path=storage={}",
                storage.path().display()
            ))
            .arg(repo_root().join("test/runtime/fixtures/durable-objects/hibernation-probe.capnp"))
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
