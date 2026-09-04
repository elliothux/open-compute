use super::*;
use open_compute_core::{DeploymentId, RequestId};
use open_compute_storage::{WorkerObservabilitySettings, WorkerOwnership, WorkerRecord};
use std::net::{IpAddr, Ipv4Addr};

fn identity() -> EffectiveIdentity {
    let account_id = AccountId::generate();
    EffectiveIdentity {
        account_id,
        worker: WorkerRecord {
            id: WorkerId::generate(),
            account_id,
            name: "observed-worker".to_owned(),
            active_deployment_id: Some(DeploymentId::generate()),
            active_version_id: None,
            do_storage_id: uuid::Uuid::now_v7().to_string(),
            route_generation: 1,
            created_at_ms: 1,
            updated_at_ms: 1,
            deleted_at_ms: None,
            ownership: WorkerOwnership::Tenant,
        },
        version_id: VersionId::generate(),
        deployment_id: Some("deployment".to_owned()),
        settings: WorkerObservabilitySettings {
            generation: 1,
            enabled: true,
            head_sampling_rate: Some(1.0),
            logs_enabled: true,
            logs_head_sampling_rate: None,
            invocation_logs: true,
            persist: true,
            updated_at_ms: 1,
        },
        secret_values: Arc::new(vec![SecretString::new("binding-secret")]),
    }
}

fn invocation() -> NewObservabilityInvocation {
    canonical_invocation(
        "collector",
        0,
        json!({
            "eventTimestamp": 1_000,
            "outcome": "ok",
            "cpuTime": 1.5,
            "wallTime": 2.5,
            "event": {"request": {
                "method": "GET",
                "url": "https://user:pass@example.com/abcdefghijklmnopqrstuvwxyz?token=binding-secret&safe=yes",
                "headers": {
                    "Authorization": "Bearer binding-secret",
                    "CF-Connecting-IP": "192.0.2.1",
                    "x-open-compute-token": "internal",
                    "x-visible": "value"
                }
            }},
            "logs": [
                {"level":"info", "message":["invoice", 7, true, null], "timestamp":1001},
                {"message":{"nested":"binding-secret"}},
                "malformed"
            ],
            "exceptions": [
                {"message":"failure binding-secret", "timestamp":1002},
                "malformed"
            ]
        }),
        &identity(),
        1_000,
        false,
        1024 * 1024,
    )
    .unwrap()
}

#[test]
fn canonical_events_redact_and_project_every_supported_source_shape() {
    let invocation = invocation();
    assert_eq!(invocation.event_type, "fetch");
    assert!(invocation.truncated);
    assert_eq!(invocation.events.len(), 4);
    let encoded = serde_json::to_string(&invocation).unwrap();
    assert!(!encoded.contains("binding-secret"));
    assert!(!encoded.contains("x-open-compute-token"));
    assert!(!encoded.contains("user:pass"));
    assert!(encoded.contains("REDACTED"));
    assert!(encoded.contains("invoice 7 true null"));
    let live = live_event(&invocation, &invocation.events[0]);
    assert_eq!(live["$workers"]["eventType"], "fetch");
    assert_eq!(live["$metadata"]["cloudService"], "workers");

    let identity = identity();
    for (event, expected) in [
        (json!({"cron":"* * * * *"}), "scheduled"),
        (json!({"queue":"jobs"}), "queue"),
        (json!({"mailFrom":"sender@example.com"}), "email"),
        (json!({"rpcMethod":"call"}), "rpc"),
        (json!({"consumedEvents":1}), "tail"),
        (json!({"scheduledTime":1}), "alarm"),
        (json!({"type":"custom-event"}), "custom-event"),
        (json!({}), "unknown"),
        (Value::Null, "unknown"),
    ] {
        let value = canonical_invocation(
            "types",
            0,
            json!({"outcome":"ok", "event":event}),
            &identity,
            2_000,
            false,
            1024 * 1024,
        )
        .unwrap();
        assert_eq!(value.event_type, expected);
    }

    for item in [
        Value::Null,
        json!({"eventTimestamp": -3_000_000_000_i64, "outcome":"ok"}),
        json!({"eventTimestamp": 3_000_000_000_i64, "outcome":"ok"}),
        json!({"event":{}}),
    ] {
        assert!(canonical_invocation("bad", 0, item, &identity, 1_000, false, 128).is_err());
    }
    let oversized = json!({"outcome":"ok", "logs": vec![Value::Null; 1025]});
    assert!(
        canonical_invocation("bad", 0, oversized, &identity, 1_000, false, 1024 * 1024).is_err()
    );
}

#[test]
fn tail_filters_cover_validation_matching_and_sampling() {
    let invocation = invocation();
    let filters = vec![
        TailFilter::Sampling(0.999999),
        TailFilter::Outcome(vec!["ok".to_owned()]),
        TailFilter::Method(vec!["get".to_owned()]),
        TailFilter::Header {
            key: "x-visible".to_owned(),
            query: "val".to_owned(),
        },
        TailFilter::ClientIp(vec![IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))]),
        TailFilter::Query("invoice".to_owned()),
        TailFilter::ScriptVersion(invocation.version_id.parse().unwrap()),
    ];
    validate_filters(&filters).unwrap();
    for filter in &filters[1..] {
        assert!(matches_tail(
            std::slice::from_ref(filter),
            &invocation,
            "session"
        ));
    }
    assert!(!matches_tail(
        &[TailFilter::Outcome(vec!["exception".to_owned()])],
        &invocation,
        "session"
    ));
    assert!(sampled("id", "namespace", 1.0));
    assert!(!sampled("id", "namespace", 0.0));
    assert!(!sampled("id", "namespace", f64::NAN));
    assert_eq!(
        sampled("id", "namespace", 0.5),
        sampled("id", "namespace", 0.5)
    );

    let invalid = [
        TailFilter::Sampling(1.0),
        TailFilter::Outcome(Vec::new()),
        TailFilter::Outcome(vec!["invalid".to_owned()]),
        TailFilter::Method(vec!["GET/POST".to_owned()]),
        TailFilter::Header {
            key: String::new(),
            query: "x".to_owned(),
        },
        TailFilter::ClientIp(Vec::new()),
        TailFilter::Query(String::new()),
    ];
    for filter in invalid {
        assert!(validate_filters(&[filter]).is_err());
    }
    assert!(
        validate_filters(&[
            TailFilter::Query("one".to_owned()),
            TailFilter::Query("two".to_owned()),
        ])
        .is_err()
    );
}

#[tokio::test]
async fn identities_tickets_overload_frames_and_helpers_are_bounded() {
    let account = AccountId::generate();
    let worker = WorkerId::generate();
    let version = VersionId::generate();
    assert_eq!(
        loader_identity(&format!("{account}/{worker}/{version}")),
        Some((account, worker, version))
    );
    assert!(loader_identity("invalid").is_none());
    assert!(loader_identity(&format!("{account}/{worker}/{version}/extra")).is_none());
    assert!(ticket_claim("id", account, worker, 10).starts_with("v1\0id\0"));
    assert!(constant_time_equal(b"same", b"same"));
    assert!(!constant_time_equal(b"short", b"longer"));
    assert_eq!(format_timestamp(0).unwrap(), "1970-01-01T00:00:00Z");
    assert!(format_timestamp(i64::MAX).is_err());
    assert!(now_ms().unwrap() > 0);
    assert_eq!(invalid().code(), ErrorCode::LimitInvalid);
    assert_eq!(stale().code(), ErrorCode::VersionInvariantViolation);
    assert_eq!(not_found().code(), ErrorCode::ResourceNotFound);
    assert_eq!(unavailable().code(), ErrorCode::PlatformUnavailable);
    assert_eq!(workers_logs_dataset(), "cloudflare-workers");

    let mut value = json!({
        "plain": "prefix-binding-secret-suffix",
        "binding-secret": ["binding-secret", {"nested": "safe"}]
    });
    redact_secret_values(&mut value, &[SecretString::new("binding-secret")]);
    let encoded = serde_json::to_string(&value).unwrap();
    assert!(!encoded.contains("binding-secret"));
    assert!(encoded.contains("prefix-REDACTED-suffix"));
    assert_eq!(redacted_url("not a url"), "https://redacted.invalid/");
    assert!(secret_header("X-Api-Key"));
    assert!(!secret_header("accept"));

    for live in [false, true] {
        let (sender, mut receiver) = mpsc::channel(4);
        let queued = Arc::new(AtomicUsize::new(0));
        enqueue_overload(&sender, &queued, true, live);
        enqueue_overload(&sender, &queued, false, live);
        assert!(queued.load(Ordering::Relaxed) > 0);
        let first = receiver.recv().await.unwrap();
        assert!(first.text.contains("dropped"));
        drop(first);
        drop(receiver.recv().await.unwrap());
        assert_eq!(queued.load(Ordering::Relaxed), 0);
    }

    let _ = RequestId::generate();
}
