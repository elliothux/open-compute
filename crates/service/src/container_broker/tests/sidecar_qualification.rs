//! Qualification failures retain their fixed phase without disclosing credentials.

use super::*;
use std::os::unix::fs::OpenOptionsExt as _;
use tracing::instrument::WithSubscriber as _;

#[tokio::test]
async fn sidecar_qualification_reports_rejection_timeout_and_cleanup_without_secrets() {
    const SECRET: &str = "private-sidecar-upstream-canary";
    for failure in ["unauthorized", "malformed", "timeout", "none"] {
        let broker = RestartBroker::new().await;
        match failure {
            "unauthorized" => {
                *broker.state.sidecar_profile_override.lock().await =
                    Some((401, SECRET.as_bytes().to_vec()));
            }
            "malformed" => {
                *broker.state.sidecar_profile_override.lock().await =
                    Some((200, SECRET.as_bytes().to_vec()));
            }
            "timeout" => {
                *broker.state.sidecar_profile_gate.lock().await =
                    Some(Arc::new(tokio::sync::Notify::new()));
            }
            _ => {}
        }
        let log_path = broker._directory.path().join("qualification.log");
        let log_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&log_path)
            .unwrap();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_target(false)
            .with_max_level(tracing::Level::WARN)
            .with_current_span(false)
            .with_span_list(false)
            .with_writer(Arc::new(log_file))
            .finish();
        let dispatch = tracing::Dispatch::new(subscriber);
        let engine =
            crate::container_broker::engine::Engine::new(&broker.config.engine_socket, "/_ping")
                .with_subscriber(dispatch.clone())
                .await
                .unwrap();
        let began = std::time::Instant::now();
        let result = engine
            .qualify(&broker.config)
            .with_subscriber(dispatch)
            .await
            .map(|contract| contract.qualified);
        if let Some(gate) = broker.state.sidecar_profile_gate.lock().await.take() {
            gate.notify_waiters();
        }
        match failure {
            "none" => assert!(result.unwrap()),
            "unauthorized" => assert!(!result.unwrap()),
            _ => assert!(result.is_err()),
        }
        if failure == "timeout" {
            assert!(began.elapsed() >= std::time::Duration::from_secs(30));
        }
        assert!(broker.state.containers.lock().await.is_empty());
        let requests = broker.state.requests.lock().await;
        let removed = requests
            .iter()
            .position(|request| request.starts_with("DELETE /containers/"))
            .unwrap();
        let path = requests[removed].strip_prefix("DELETE ").unwrap();
        assert_eq!(
            requests[removed + 1..]
                .iter()
                .find(|request| request.starts_with("GET /containers/")),
            Some(&format!("GET {path}/json"))
        );
        let log = std::fs::read_to_string(&log_path).unwrap();
        let credentials = broker.state.credentials.lock().await;
        let credentials = credentials.as_ref().unwrap();
        for secret in [
            SECRET,
            credentials.control_token.as_str(),
            credentials.callback_token.as_str(),
            broker.config.engine_socket.to_str().unwrap(),
        ] {
            assert!(!log.contains(secret));
        }
        let events: Vec<Value> = log
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|value| value["fields"]["message"] == "Container sidecar qualification failed")
            .collect();
        if failure == "none" {
            assert!(events.is_empty());
        } else {
            assert_eq!(events.len(), 1);
            assert_eq!(events[0]["fields"]["phase"], "profile");
            assert_eq!(events[0]["fields"]["timeout"], failure == "timeout");
        }
    }
}

#[tokio::test]
async fn sidecar_qualification_checks_all_denials_within_the_original_deadline() {
    let broker = RestartBroker::new().await;
    // Each response fits the existing three-second request budget; serial checks exceed thirty.
    *broker.state.sidecar_auth_delay.lock().await = Some(std::time::Duration::from_millis(2_600));
    let engine =
        crate::container_broker::engine::Engine::new(&broker.config.engine_socket, "/_ping")
            .await
            .unwrap();
    let began = std::time::Instant::now();
    let result = engine.qualify(&broker.config).await;
    assert!(broker.state.containers.lock().await.is_empty());
    assert!(result.unwrap().qualified);
    assert!(began.elapsed() < std::time::Duration::from_secs(30));
    assert_eq!(
        broker.state.sidecar_auth_rejections.load(Ordering::Relaxed),
        12
    );
}

#[tokio::test]
async fn sidecar_qualification_rejects_one_successful_unauthorized_method() {
    let broker = RestartBroker::new().await;
    // The callback probe must reject both missing and wrong credentials too.
    broker
        .state
        .sidecar_callback_auth_status
        .store(200, Ordering::Relaxed);
    let engine =
        crate::container_broker::engine::Engine::new(&broker.config.engine_socket, "/_ping")
            .await
            .unwrap();
    assert!(!engine.qualify(&broker.config).await.unwrap().qualified);
    assert!(broker.state.containers.lock().await.is_empty());
}
