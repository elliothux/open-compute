//! Native Service Binding handoff to a hibernatable Durable Object WebSocket.

use super::p3_services_support::Harness;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use hyper_util::client::legacy::{Client, connect::HttpConnector};
use hyper_util::rt::{TokioExecutor, TokioIo};
use open_compute_core::{AccountId, BindingKind, RequestId, ResourceId, VersionId, WorkerId};
use open_compute_service::runtime_bridge::{DispatchTarget, WorkerdTransport};
use open_compute_service::service_invocations::ServiceInvocationRegistry;
use open_compute_storage::{DO_NAMESPACE_SCHEMA_VERSION, VersionRecord};
use open_compute_workers::{
    CreateResourceOutcome, CreateResourceRequest, DurableObjectResourceDriver, ResourceController,
    ResourcePins, VersionPins,
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(super) fn create_namespace(
    harness: &Harness,
    account_id: AccountId,
    worker_id: WorkerId,
) -> ResourceId {
    let driver = DurableObjectResourceDriver::new(&harness.storage, worker_id, "SocketRoom");
    match ResourceController::new(&harness.storage, ResourcePins::new(), driver)
        .create(&CreateResourceRequest {
            account_id,
            kind: BindingKind::DoNamespace,
            name: "service-websocket-objects".to_owned(),
            idempotency_key: "service-websocket-objects".to_owned(),
            driver_schema_version: DO_NAMESPACE_SCHEMA_VERSION,
            request_id: RequestId::generate(),
            now_ms: 9,
        })
        .unwrap()
    {
        CreateResourceOutcome::Applied(value) => value.resource_id,
        CreateResourceOutcome::Replay(_) => panic!("unexpected namespace replay"),
    }
}

pub(super) async fn verify(
    transport: &WorkerdTransport,
    account_id: AccountId,
    caller_id: WorkerId,
    caller_version: &VersionRecord,
    target_version_id: VersionId,
    version_pins: &VersionPins,
    service_invocations: &ServiceInvocationRegistry,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let socket_transport = transport.clone();
    let socket_target = DispatchTarget {
        account_id,
        worker_id: caller_id,
        version_id: caller_version.id,
        worker_code_sha256: hex::encode(caller_version.worker_code_sha256),
        entrypoint: None,
        route_generation: 1,
        request_id: RequestId::generate(),
    };
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().fallback(move |request: axum::extract::Request| {
                let transport = socket_transport.clone();
                let target = socket_target.clone();
                async move { transport.dispatch(target, request).await.unwrap() }
            }),
        )
        .await
        .unwrap();
    });
    let client: Client<HttpConnector, Body> = Client::builder(TokioExecutor::new()).build_http();
    for path in ["/socket", "/named-socket"] {
        let request = Request::builder()
            .uri(format!("http://{address}{path}"))
            .header(header::HOST, "caller.example")
            .header(header::CONNECTION, "Upgrade")
            .header(header::UPGRADE, "websocket")
            .header(header::SEC_WEBSOCKET_VERSION, "13")
            .header(header::SEC_WEBSOCKET_KEY, "AAECAwQFBgcICQoLDA0ODw==")
            .body(Body::empty())
            .unwrap();
        let mut response = client.request(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS, "{path}");
        let mut socket = TokioIo::new(hyper::upgrade::on(&mut response).await.unwrap());
        assert!(version_pins.count(target_version_id) > 0);
        if path == "/socket" {
            tokio::time::sleep(Duration::from_secs(65)).await;
            assert!(
                version_pins.count(target_version_id) > 0,
                "hibernatable Service WebSocket must retain its target pin past the call deadline"
            );
            assert_ne!(service_invocations.counts(), (0, 0, 0));
        }
        for opcode in [0x81, 0x82] {
            socket
                .write_all(&[opcode, 0x82, 1, 2, 3, 4, b'h' ^ 1, b'i' ^ 2])
                .await
                .unwrap();
            let mut bytes = [0; 4];
            tokio::time::timeout(Duration::from_secs(5), socket.read_exact(&mut bytes))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(bytes, [opcode, 2, b'h', b'i']);
        }
        socket
            .write_all(&[0x88, 0x82, 1, 2, 3, 4, 3 ^ 1, 232 ^ 2])
            .await
            .unwrap();
        let mut rest = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), socket.read_to_end(&mut rest))
            .await
            .unwrap()
            .unwrap();
        drop(socket);
        super::wait_pin_count(version_pins, service_invocations, target_version_id, 0).await;
        super::wait_service_counts(service_invocations, (0, 0, 0)).await;
    }
    server.abort();
    let _ = server.await;
}
