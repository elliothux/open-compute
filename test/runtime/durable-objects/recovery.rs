//! Retained product recovery invariants from the retired native-facet investigation.
//! The owning P0.7 Gate supplies the real loader, persisted authority, and stock workerd.

use super::*;

pub(super) async fn check(
    transport: &WorkerdTransport,
    supervisor: &WorkerdSupervisor,
    account: AccountId,
    worker: WorkerId,
    deployment: &DeploymentRecord,
    generation: u64,
) {
    let failed = dispatch(
        transport,
        account,
        worker,
        deployment,
        generation,
        "/committed-failure?name=confirmed-failure",
    )
    .await;
    assert_eq!((failed.status, failed.body.as_str()), (503, "failed"));
    let confirmed = dispatch(
        transport,
        account,
        worker,
        deployment,
        generation,
        "/rpc?name=confirmed-failure",
    )
    .await;
    assert_eq!((confirmed.status, confirmed.body.as_str()), (200, "A:1"));

    let writes = futures::future::join_all((0..8).map(|_| {
        dispatch(
            transport,
            account,
            worker,
            deployment,
            generation,
            "/increment?name=concurrent-recovery",
        )
    }))
    .await;
    assert!(writes.iter().all(|result| result.status == 200));
    let values: std::collections::BTreeSet<_> =
        writes.iter().map(|result| result.body.as_str()).collect();
    assert_eq!(values.len(), 8, "same-object writes cannot lose updates");

    let pending = tokio::spawn({
        let transport = transport.clone();
        let deployment = deployment.clone();
        async move {
            transport
                .dispatch(
                    DispatchTarget {
                        account_id: account,
                        worker_id: worker,
                        deployment_id: deployment.id,
                        worker_code_sha256: hex::encode(deployment.worker_code_sha256),
                        entrypoint: None,
                        route_generation: i64::try_from(generation).unwrap(),
                        request_id: RequestId::generate(),
                    },
                    Request::builder()
                        .method("POST")
                        .uri("/held-write?name=held-recovery")
                        .header(header::HOST, "do.test")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
        }
    });
    // Observe a synced write through a separate RPC instead of sleeping before SIGKILL.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let status = dispatch(
                transport,
                account,
                worker,
                deployment,
                generation,
                "/held-status?name=held-recovery",
            )
            .await;
            assert_eq!(status.status, 200);
            if status.body == "true" {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("held write must enter the confirmed, pre-response phase");
    assert!(
        !pending.is_finished(),
        "crash must interrupt an in-flight response"
    );
    let old_pid = supervisor.snapshot().pid.unwrap();
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(old_pid).unwrap(),
        rustix::process::Signal::KILL,
    )
    .unwrap();
    wait_pid_change(supervisor, old_pid, Duration::from_secs(30)).await;
    let interrupted = tokio::time::timeout(Duration::from_secs(10), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        interrupted.unwrap_err().code(),
        open_compute_core::ErrorCode::RuntimeUnavailable
    );
    for (name, expected) in [
        ("confirmed-failure", "A:1"),
        ("concurrent-recovery", "A:8"),
        ("held-recovery", "A:1"),
    ] {
        let recovered = dispatch(
            transport,
            account,
            worker,
            deployment,
            generation,
            &format!("/rpc?name={name}"),
        )
        .await;
        assert_eq!((recovered.status, recovered.body.as_str()), (200, expected));
        let rolled_back = dispatch(
            transport,
            account,
            worker,
            deployment,
            generation,
            &format!("/rollback?name={name}"),
        )
        .await;
        assert_eq!(rolled_back.status, 200);
        assert!(
            rolled_back.body.starts_with("true:"),
            "rollback must remain usable after crash"
        );
    }
}
