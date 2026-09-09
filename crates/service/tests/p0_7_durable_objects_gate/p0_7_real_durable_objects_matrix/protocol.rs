use super::*;

pub(super) async fn verify_identity(case: &MatrixCase<'_>) -> String {
    let transport = case.transport;
    let _supervisor = case.supervisor;
    let storage = case.storage;
    let account = case.account;
    let worker_id = case.worker.id;
    let counter = case.counter;
    let _other = case.other;
    let _output_queue_resource = case.output_queue_resource;
    let version_a = case.version_a;
    let generation_a = case.generation_a;
    let ids = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/ids",
    )
    .await;
    assert_eq!(ids.status, 200, "{}", ids.body);
    let identity: serde_json::Value = serde_json::from_str(&ids.body).unwrap();
    let named_id = identity["named"].as_str().unwrap();
    assert_eq!(named_id.len(), 64);
    assert_eq!(identity["named"], identity["namedAgain"]);
    assert_ne!(identity["named"], identity["unique"]);
    assert_eq!(identity["crossNamespaceRejected"], true);
    assert_eq!(identity["uppercaseRejected"], true);
    assert_eq!(identity["invalidHintRejected"], true);
    assert_eq!(identity["locationAccepted"], true);
    assert_eq!(identity["jurisdiction"], "eu");
    assert_eq!(identity["namedJurisdiction"], "eu");
    assert_eq!(identity["jurisdictionRoundTrip"], true);
    assert_eq!(identity["jurisdictionChangesId"], true);
    assert_eq!(identity["unscopedGetAcceptsJurisdiction"], true);
    assert_eq!(identity["nullishJurisdiction"], true);
    assert_eq!(identity["forgedRejected"], true);
    assert_eq!(identity["forgedBridgeRejected"], true);
    assert_eq!(identity["mutatedIntrinsicNamed"], identity["named"]);
    assert!(
        DurableObjectId::from_str(named_id)
            .unwrap()
            .belongs_to(counter)
    );
    let (prefix, name_key) = DurableObjectRepository::new(storage)
        .facade_identity(counter)
        .unwrap();
    let mut expected = Vec::from(prefix);
    let mut mac = <Hmac<Sha256>>::new_from_slice(&name_key).unwrap();
    mac.update(b"\x6e\x00alpha");
    let named_body = mac.finalize().into_bytes();
    let mut payload = Vec::with_capacity(24);
    payload.push(0xa0);
    payload.extend_from_slice(&named_body[..15]);
    let mut tag = <Hmac<Sha256>>::new_from_slice(&name_key).unwrap();
    tag.update(&payload);
    payload.extend_from_slice(&tag.finalize().into_bytes()[..8]);
    expected.extend_from_slice(&payload);
    assert_eq!(named_id, hex::encode(expected));
    named_id.to_owned()
}

pub(super) async fn verify_fetch_and_rpc(case: &MatrixCase<'_>) {
    let transport = case.transport;
    let supervisor = case.supervisor;
    let _storage = case.storage;
    let account = case.account;
    let worker_id = case.worker.id;
    let _counter = case.counter;
    let _other = case.other;
    let _output_queue_resource = case.output_queue_resource;
    let version_a = case.version_a;
    let generation_a = case.generation_a;
    let first = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/increment?name=alpha",
    )
    .await;
    if first.status != 200 {
        let failed_pid = supervisor.snapshot().pid.unwrap();
        supervisor.report_unhealthy();
        wait_pid_change(supervisor, failed_pid, Duration::from_secs(30)).await;
        panic!(
            "first DO dispatch failed: {}; diagnostics={:?}",
            first.body,
            supervisor.last_diagnostics()
        );
    }
    assert_eq!(first.body, "A:1");
    let second = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/rpc?name=alpha",
    )
    .await;
    if second.status != 200 {
        let failed_pid = supervisor.snapshot().pid.unwrap();
        supervisor.report_unhealthy();
        wait_pid_change(supervisor, failed_pid, Duration::from_secs(30)).await;
        panic!(
            "DO RPC failed: {}; diagnostics={:?}",
            second.body,
            supervisor.last_diagnostics()
        );
    }
    assert_eq!(second.body, "A:1");
    let binary_rpc = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/rpc-binary?name=alpha",
    )
    .await;
    assert_eq!(
        (binary_rpc.status, binary_rpc.body.as_str()),
        (200, "4,5,6")
    );
    let connect = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/connect?name=alpha",
    )
    .await;
    assert_eq!(
        (connect.status, connect.body.as_str()),
        (200, "4,5,6"),
        "diagnostics={:?}",
        supervisor.last_diagnostics()
    );
    let connect_ipv6 = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/connect-ipv6?name=alpha",
    )
    .await;
    assert_eq!(
        (connect_ipv6.status, connect_ipv6.body.as_str()),
        (200, "10,11,12"),
        "diagnostics={:?}",
        supervisor.last_diagnostics()
    );
    let structured_rpc = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/rpc-structured?name=alpha",
    )
    .await;
    assert_eq!(structured_rpc.status, 200, "{}", structured_rpc.body);
    let structured: serde_json::Value = serde_json::from_str(&structured_rpc.body).unwrap();
    assert_eq!(structured["time"], "2026-08-30T00:00:00.000Z");
    for member in [
        "bigint", "map", "regexp", "error", "typed", "view", "buffer", "headers", "request",
        "response",
    ] {
        assert_eq!(structured[member], true, "{member}: {structured}");
    }
    let stream_rpc = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/rpc-stream?name=alpha",
    )
    .await;
    assert_eq!(
        (stream_rpc.status, stream_rpc.body.as_str()),
        (200, "7,8,9")
    );
    let writable_rpc = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/rpc-writable?name=alpha",
    )
    .await;
    assert_eq!(
        (writable_rpc.status, writable_rpc.body.as_str()),
        (200, "10,11")
    );
    let capability_rpc = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/rpc-capability?name=alpha",
    )
    .await;
    assert_rpc_capability(&capability_rpc, "A");
}

pub(super) async fn verify_rpc_edges(case: &MatrixCase<'_>) {
    let transport = case.transport;
    let supervisor = case.supervisor;
    let _storage = case.storage;
    let account = case.account;
    let worker_id = case.worker.id;
    let _counter = case.counter;
    let _other = case.other;
    let _output_queue_resource = case.output_queue_resource;
    let version_a = case.version_a;
    let generation_a = case.generation_a;
    let property_rpc = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/rpc-property?name=alpha",
    )
    .await;
    assert_eq!(property_rpc.status, 200, "{}", property_rpc.body);
    let property: serde_json::Value = serde_json::from_str(&property_rpc.body).unwrap();
    assert_eq!(property["regular"], "A:property");
    assert_eq!(property["punctuation"], "A:punctuation");
    assert_eq!(property["method"], "A:punctuation-method");
    let property_error = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/rpc-property-error?name=alpha",
    )
    .await;
    assert_eq!(
        (property_error.status, property_error.body.as_str()),
        (200, "true")
    );
    let callback_rpc = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/rpc-callback?name=alpha",
    )
    .await;
    assert_eq!(callback_rpc.status, 200, "{}", callback_rpc.body);
    let callback: serde_json::Value = serde_json::from_str(&callback_rpc.body).unwrap();
    assert_eq!(callback["target"], "target:ok");
    assert_eq!(callback["callback"], "function:ok");
    let clone_error = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/rpc-clone-error?name=alpha",
    )
    .await;
    assert_eq!(
        (clone_error.status, clone_error.body.as_str()),
        (200, "true")
    );
    let capability_error = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/rpc-capability-error?name=alpha",
    )
    .await;
    assert_eq!(
        (capability_error.status, capability_error.body.as_str()),
        (200, "true"),
        "tenant RpcTarget exceptions must be sanitized at the trust boundary"
    );
    let rpc_error = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/rpc-error?name=alpha",
    )
    .await;
    assert_eq!((rpc_error.status, rpc_error.body.as_str()), (200, "true"));
    let rollback = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/rollback?name=alpha",
    )
    .await;
    assert_eq!((rollback.status, rollback.body.as_str()), (200, "true:1"));
    let websocket = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/websocket?name=alpha",
    )
    .await;
    if websocket.status != 200 {
        let failed_pid = supervisor.snapshot().pid.unwrap();
        supervisor.report_unhealthy();
        wait_pid_change(supervisor, failed_pid, Duration::from_secs(30)).await;
        panic!(
            "DO websocket failed: {}; diagnostics={:?}",
            websocket.body,
            supervisor.last_diagnostics()
        );
    }
    assert_eq!(websocket.body, "text:true,binary:true");
}

pub(super) async fn verify_ordering(case: &MatrixCase<'_>) {
    let transport = case.transport;
    let _supervisor = case.supervisor;
    let _storage = case.storage;
    let account = case.account;
    let worker_id = case.worker.id;
    let _counter = case.counter;
    let _other = case.other;
    let _output_queue_resource = case.output_queue_resource;
    let version_a = case.version_a;
    let generation_a = case.generation_a;
    let ordered = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/order?name=ordered",
    )
    .await;
    assert_eq!(ordered.status, 200, "{}", ordered.body);
    let order: Vec<String> = serde_json::from_str(&ordered.body).unwrap();
    let first_start = order.iter().position(|item| item == "first:start").unwrap();
    let second_start = order
        .iter()
        .position(|item| item == "second:start")
        .unwrap();
    assert!(first_start < second_start, "same-stub E-order: {order:?}");

    let overlap = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/fetch-overlap?name=fetch-overlap",
    )
    .await;
    assert_eq!(overlap.status, 200, "{}", overlap.body);
    let order: Vec<String> = serde_json::from_str(&overlap.body).unwrap();
    assert_eq!(
        order,
        [
            "fetch-first:start",
            "fetch-second:start",
            "fetch-second:end",
            "fetch-first:end"
        ],
        "same-stub fetches preserve start order without waiting for completion"
    );

    for expected in ["A:1", "A:2"] {
        let idle = dispatch(
            transport,
            account,
            worker_id,
            version_a,
            generation_a,
            "/?name=idle-order",
        )
        .await;
        assert_eq!(idle.status, 200, "{}", idle.body);
        assert_eq!(idle.body, expected);
        if expected == "A:1" {
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    }

    let cross_ordered = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/cross-order?name=cross-ordered",
    )
    .await;
    assert_eq!(cross_ordered.status, 200, "{}", cross_ordered.body);
    let cross_order: serde_json::Value = serde_json::from_str(&cross_ordered.body).unwrap();
    assert_eq!(cross_order["echoed"], true, "{cross_order}");
    let starts = cross_order["order"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|value| value.as_str())
        .filter(|value| value.ends_with(":start"))
        .collect::<Vec<_>>();
    assert_eq!(
        starts,
        [
            "rpc-first:start",
            "fetch-second:start",
            "connect:start",
            "rpc-fourth:start",
        ],
        "cross-surface same-stub E-order: {cross_order}"
    );

    let order_error = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/order-error?name=order-error",
    )
    .await;
    assert_eq!(order_error.status, 200, "{}", order_error.body);
    let order_error: serde_json::Value = serde_json::from_str(&order_error.body).unwrap();
    for key in ["failed", "fetched", "rpc"] {
        assert_eq!(order_error[key], true, "{key}: {order_error}");
    }
}

pub(super) async fn verify_storage_and_parallelism(case: &MatrixCase<'_>) {
    let transport = case.transport;
    let _supervisor = case.supervisor;
    let _storage = case.storage;
    let account = case.account;
    let worker_id = case.worker.id;
    let _counter = case.counter;
    let _other = case.other;
    let _output_queue_resource = case.output_queue_resource;
    let version_a = case.version_a;
    let generation_a = case.generation_a;
    let storage_matrix = dispatch(
        transport,
        account,
        worker_id,
        version_a,
        generation_a,
        "/storage?name=storage-matrix",
    )
    .await;
    assert_eq!(storage_matrix.status, 200, "{}", storage_matrix.body);
    hibernation::storage_members(&serde_json::from_str(&storage_matrix.body).unwrap());
    hibernation::facets(transport, account, worker_id, version_a, generation_a).await;

    let parallel_start = Instant::now();
    let (left, right) = tokio::join!(
        dispatch(
            transport,
            account,
            worker_id,
            version_a,
            generation_a,
            "/hold?name=left&ms=250&window=1",
        ),
        dispatch(
            transport,
            account,
            worker_id,
            version_a,
            generation_a,
            "/hold?name=right&ms=250&window=1",
        ),
    );
    assert_eq!(
        (left.status, right.status),
        (200, 200),
        "left={} right={}",
        left.body,
        right.body
    );
    let left_hold: serde_json::Value = serde_json::from_str(&left.body).expect("left hold json");
    let right_hold: serde_json::Value = serde_json::from_str(&right.body).expect("right hold json");
    let left_t0 = left_hold["t0"].as_i64().expect("left t0");
    let left_t1 = left_hold["t1"].as_i64().expect("left t1");
    let right_t0 = right_hold["t0"].as_i64().expect("right t0");
    let right_t1 = right_hold["t1"].as_i64().expect("right t1");
    assert!(
        left_t0 < right_t1 && right_t0 < left_t1,
        "parallel DO holds must overlap: left={left_hold} right={right_hold} wall={:?}",
        parallel_start.elapsed(),
    );
}
