//! Controlled HTTP clients with explicit timeout/cancellation and fixture ownership.

use super::PYTHON;
use super::python_support::fixture::Fixture;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

#[derive(Default)]
struct State {
    pending: AtomicUsize,
    admitted: AtomicUsize,
    slow_completed: AtomicUsize,
    echoes: AtomicUsize,
    changed: tokio::sync::Notify,
}

struct Pending(Arc<State>);

impl Drop for Pending {
    fn drop(&mut self) {
        self.0.pending.fetch_sub(1, Ordering::SeqCst);
        self.0.changed.notify_waiters();
    }
}

pub(super) struct Server {
    address: SocketAddr,
    state: Arc<State>,
    stop: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

impl Server {
    pub(super) async fn start() -> Self {
        let state = Arc::new(State::default());
        let slow_state = state.clone();
        let pending_state = state.clone();
        let echo_state = state.clone();
        let router = axum::Router::new()
            .route(
                "/echo",
                post(move |headers: HeaderMap, body: Bytes| {
                    let state = echo_state.clone();
                    async move {
                        state.echoes.fetch_add(1, Ordering::SeqCst);
                        assert!(headers["x-caller"].to_str().unwrap().contains('/'));
                        (
                            StatusCode::ACCEPTED,
                            [
                                ("x-fixture", "python-http-clients"),
                                ("content-type", "text/plain; charset=utf-8"),
                            ],
                            body,
                        )
                    }
                }),
            )
            .route(
                "/error",
                post(|| async { (StatusCode::UNPROCESSABLE_ENTITY, "application failure") }),
            )
            .route(
                "/stream",
                get(|| async {
                    let chunks = futures::stream::iter([
                        Ok::<_, std::io::Error>(Bytes::from_static("first/µ/".as_bytes())),
                        Ok(Bytes::from_static("☁/last".as_bytes())),
                    ]);
                    Response::new(Body::from_stream(chunks))
                }),
            )
            .route(
                "/slow",
                get(move || slow(slow_state.clone())).post({
                    let state = state.clone();
                    move || slow(state.clone())
                }),
            )
            .route(
                "/pending",
                get(move || {
                    let state = pending_state.clone();
                    async move {
                        tokio::time::timeout(Duration::from_secs(3), async {
                            loop {
                                let notified = state.changed.notified();
                                if state.pending.load(Ordering::SeqCst) > 0 {
                                    break;
                                }
                                notified.await;
                            }
                        })
                        .await
                        .unwrap();
                        "pending"
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        Self {
            address,
            state,
            stop,
            task,
        }
    }

    async fn drained(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let notified = self.state.changed.notified();
                if self.state.pending.load(Ordering::SeqCst) == 0 {
                    break;
                }
                notified.await;
            }
        })
        .await
        .expect("HTTP fixture retained a pending operation");
    }

    pub(super) async fn finish(self) {
        self.drained().await;
        self.stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), self.task)
            .await
            .unwrap()
            .unwrap();
        assert!(tokio::net::TcpListener::bind(self.address).await.is_ok());
    }
}

async fn slow(state: Arc<State>) -> &'static str {
    state.pending.fetch_add(1, Ordering::SeqCst);
    state.admitted.fetch_add(1, Ordering::SeqCst);
    state.changed.notify_waiters();
    let _pending = Pending(state.clone());
    // Deliberately delayed fixture response; client deadlines must expire first.
    tokio::time::sleep(Duration::from_secs(1)).await;
    state.slow_completed.fetch_add(1, Ordering::SeqCst);
    "late response"
}

pub(super) async fn matrix(fixture: &Fixture, server: &Server) {
    let base = format!("http://{}/", server.address);
    let mut evidence = Vec::new();
    for client in ["requests", "httpx-sync", "httpx-async"] {
        for case in ["echo", "error", "stream", "timeout"] {
            let admitted = server.state.admitted.load(Ordering::SeqCst);
            let slow_completed = server.state.slow_completed.load(Ordering::SeqCst);
            let start = Instant::now();
            let result = call(fixture, client, case, &base).await;
            assert_eq!(result["requests"], "2.33.1");
            assert_eq!(result["httpx"], "0.28.1");
            match case {
                "echo" => {
                    assert_eq!(result["status"], 202);
                    assert_eq!(result["header"], "python-http-clients");
                    let body = result["body"].as_str().unwrap();
                    assert!(body.starts_with("µ☁/"), "{result}");
                    assert!(body.ends_with(client));
                }
                "error" => {
                    assert_eq!(result["failed"], true, "{result}");
                    assert_eq!(
                        result["errorType"],
                        if client == "requests" {
                            "HTTPError"
                        } else {
                            "HTTPStatusError"
                        }
                    );
                }
                "stream" => {
                    assert_eq!(
                        result["status"],
                        200,
                        "{client}/{case} after {:?}: {result}",
                        start.elapsed()
                    );
                    assert_eq!(result["body"], "first/µ/☁/last");
                }
                "timeout" => {
                    if client == "requests" {
                        assert_eq!(result["failed"], true, "{result}");
                        assert_eq!(result["errorType"], "ConnectionError", "{result}");
                        assert!(
                            result["errorChain"]
                                .as_array()
                                .unwrap()
                                .contains(&json!("_TimeoutError")),
                            "{result}"
                        );
                    } else {
                        assert_eq!(result["timeout"], true, "{result}");
                    }
                    assert!(start.elapsed() < Duration::from_secs(5));
                    assert_eq!(server.state.admitted.load(Ordering::SeqCst), admitted + 1);
                    assert_eq!(
                        server.state.slow_completed.load(Ordering::SeqCst),
                        slow_completed,
                        "client deadline did not expire before the delayed response"
                    );
                }
                _ => unreachable!(),
            }
            server.drained().await;
            evidence.push(result);
        }
        let result = call(fixture, client, "echo", &base).await;
        assert_eq!(
            result["status"], 202,
            "HTTP client failed to recover after timeout"
        );
    }
    let before = server.state.admitted.load(Ordering::SeqCst);
    let cancelled = call(fixture, "httpx-async", "cancel", &base).await;
    assert_eq!(cancelled["cancelled"], true, "{cancelled}");
    assert_eq!(server.state.admitted.load(Ordering::SeqCst), before + 1);
    server.drained().await;
    assert_eq!(
        call(fixture, "httpx-async", "echo", &base).await["status"],
        202
    );
    // Refusal is a real closed port, not a synthetic library exception.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let refused = format!("http://{}/", listener.local_addr().unwrap());
    drop(listener);
    for client in ["requests", "httpx-sync", "httpx-async"] {
        let result = call(fixture, client, "echo", &refused).await;
        assert_eq!(result["failed"], true, "{result}");
        assert_eq!(call(fixture, client, "echo", &base).await["status"], 202);
        evidence.push(result);
    }
    println!(
        "python-http-clients-evidence: {}",
        json!({"cases": evidence, "asyncCancellation": cancelled, "pendingOperationsAfterCleanup": server.state.pending.load(Ordering::SeqCst), "immediateTransportAbortOnTaskCancellation": "not_claimed"})
    );
}

async fn call(fixture: &Fixture, client: &str, case: &str, base: &str) -> Value {
    fixture
        .invoke(
            PYTHON,
            &format!("/http-client?client={client}&case={case}&url={base}"),
        )
        .await
}

pub(super) async fn budget(fixture: &Fixture, server: &Server) {
    let base = format!("http://{}/", server.address);
    for client in ["requests", "httpx-sync", "httpx-async"] {
        let before = server.state.echoes.load(Ordering::SeqCst);
        let result = call(fixture, client, "budget", &base).await;
        assert_eq!(result["completed"], 2, "{result}");
        assert_eq!(result["limited"], true, "{result}");
        assert_eq!(server.state.echoes.load(Ordering::SeqCst), before + 2);
        assert_eq!(call(fixture, client, "echo", &base).await["status"], 202);
        println!("python-http-client-subrequest-budget: {result}");
    }
}
