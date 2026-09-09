use super::*;

pub(super) async fn run() {
    let mut harness = Harness::start().await;
    let db = Connection::open(
        harness
            .storage
            .data_dir()
            .root()
            .join("durable-waiting-probe.sqlite"),
    )
    .unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
        CREATE TABLE runs(id TEXT PRIMARY KEY,state TEXT NOT NULL,token TEXT);
        CREATE TABLE steps(id TEXT NOT NULL REFERENCES runs(id),ordinal INTEGER NOT NULL,descriptor TEXT NOT NULL,
            state TEXT NOT NULL,token TEXT,deadline INTEGER NOT NULL,output TEXT,code TEXT,attempt INTEGER NOT NULL,PRIMARY KEY(id,ordinal));").unwrap();
    let db = Arc::new(Mutex::new(db));
    let backend = Backend {
        auth: harness.binding_auth.clone(),
        db: db.clone(),
    };
    let listener = harness.binding_listener.take().unwrap();
    let mut shutdown = harness.shutdown.subscribe();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/internal/workflows/runs/{operation}", post(handle))
                .with_state(backend),
        )
        .with_graceful_shutdown(async move {
            let _ = shutdown.changed().await;
        })
        .await
        .unwrap();
    });
    let target = harness.deploy(SOURCE, "Flow").await;
    let scheduler = Arc::new(
        SchedulerStore::open(
            &harness.storage.data_dir().ensure_scheduler_db().unwrap(),
            5000,
            now(),
        )
        .unwrap(),
    );
    let account = harness.storage.identity().default_account_id;
    let repository = WorkflowRepository::new(harness.storage.db());
    let definition = repository
        .create_definition(account, "probe", now())
        .unwrap();
    let api = WorkflowApiState::new(
        harness.storage.clone(),
        scheduler,
        harness.transport.clone(),
        Default::default(),
    );
    let current = api
        .create_version(account, definition.id, target.version_id, "Flow".into())
        .await
        .unwrap();
    assert_eq!(current.state, VersionState::Ready);
    assert_eq!(current.target.capability_version, 1);
    assert_eq!(
        repository
            .version(account, current.target.workflow_version_id)
            .unwrap()
            .target,
        current.target
    );
    let version = current.target;
    harness.transport.probe_workflow(&version).await.unwrap();
    if let Some(payload) = raw_tcp_fixture_payload() {
        let mut request = envelope(&db.lock().unwrap(), "rawTcp");
        request.payload_base64 = encode_workflow_json(&payload);
        let response = harness
            .transport
            .dispatch_workflow(&version, &request, Duration::from_secs(10))
            .await
            .unwrap();
        let WorkflowOutcome::Complete { output_base64, .. } = response.result else {
            panic!("expected raw TCP Workflow completion");
        };
        assert_eq!(
            decode_workflow_json(&output_base64),
            json!({"bytes": 4.0, "denied": true})
        );
    }
    for mode in ["hostileWait", "hostileRetry"] {
        let mut request = envelope(&db.lock().unwrap(), mode);
        let first = harness
            .transport
            .dispatch_workflow(&version, &request, Duration::from_secs(10))
            .await;
        assert!(
            first.is_ok(),
            "mode={mode}: {first:?} {:?}",
            harness.supervisor.last_diagnostics()
        );
        let first = first.unwrap();
        assert!(matches!(first.result, WorkflowOutcome::Suspended { .. }));
        assert!(!first.drain_incomplete);
        {
            let db = db.lock().unwrap();
            if mode == "hostileWait" {
                let event = encode_workflow_json(
                    &json!({"type":"approved","payload":7,"timestampMs":now()}),
                );
                db.execute(
                    "UPDATE steps SET state='complete',output=?2 WHERE id=?1 AND state='waiting'",
                    params![request.fence.instance_id.to_string(), event],
                )
                .unwrap();
            }
            assert_eq!(
                db.execute(
                    "UPDATE runs SET state='running',token=?2 WHERE id=?1 AND state='waiting'",
                    params![request.fence.instance_id.to_string(), "33".repeat(32)]
                )
                .unwrap(),
                1
            );
        }
        request.fence.run_token = WorkflowToken::from_bytes([0x33; 32]);
        let replay = harness
            .transport
            .dispatch_workflow(&version, &request, Duration::from_secs(10))
            .await
            .unwrap();
        let WorkflowOutcome::Complete { output_base64, .. } = replay.result else {
            panic!("expected replay completion");
        };
        let output = decode_workflow_json(&output_base64);
        assert_eq!(output["observedPrivateGrant"], false, "{mode}");
        if mode == "hostileWait" {
            assert_eq!(output["date"], true);
            assert_eq!(output["payload"], 7.0);
        } else {
            assert_eq!(output["attempt"], 2.0);
        }
    }
    let mut sleeping = None;
    for mode in [
        "normal",
        "sleep",
        "catch",
        "forged",
        "oversizedFinal",
        "oversizedStep",
        "forgedSerialization",
        "context",
        "parallel",
        "nonretryable",
        "hostile",
        "lateResolve",
        "lateReject",
        "timeout",
    ] {
        let request = envelope(&db.lock().unwrap(), mode);
        let started = Instant::now();
        let response = harness
            .transport
            .dispatch_workflow(&version, &request, Duration::from_secs(40))
            .await;
        assert!(
            response.is_ok(),
            "mode={mode}: {response:?} {:?}",
            harness.supervisor.last_diagnostics()
        );
        let response = response.unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(if mode == "timeout" { 35 } else { 3 }),
            "long waiting must end the actual dispatch RPC"
        );
        match (&response.result, mode) {
            (WorkflowOutcome::Suspended { final_ordinal }, "sleep" | "catch") => {
                assert_eq!(*final_ordinal, 1);
                let db = db.lock().unwrap();
                let (state, token): (String, Option<String>) = db
                    .query_row(
                        "SELECT state,token FROM runs WHERE id=?1",
                        [request.fence.instance_id.to_string()],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .unwrap();
                assert_eq!(state, "waiting");
                assert!(token.is_none());
                let count: i64 = db
                    .query_row(
                        "SELECT count(*) FROM steps WHERE id=?1",
                        [request.fence.instance_id.to_string()],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(count, 1, "catch cannot acquire another grant");
                assert!(!response.drain_incomplete);
                if mode == "sleep" {
                    sleeping = Some(request);
                }
            }
            (WorkflowOutcome::Errored { error_code, .. }, "forged") => {
                assert_eq!(error_code, "WORKFLOW_EXECUTION_FAILED");
            }
            (WorkflowOutcome::Errored { error_code, .. }, "oversizedFinal" | "oversizedStep") => {
                assert_eq!(error_code, "WORKFLOW_RESULT_TOO_LARGE");
            }
            (WorkflowOutcome::Errored { error_code, .. }, "forgedSerialization") => {
                assert_eq!(error_code, "WORKFLOW_SERIALIZATION_UNSUPPORTED");
            }
            (WorkflowOutcome::Complete { output_base64, .. }, _) => {
                let output = decode_workflow_json(output_base64);
                match mode {
                    "normal" => assert_eq!(output, 7.0),
                    "context" => assert_eq!(output, json!({"frozen":true,"attempt":1.0})),
                    "parallel" => assert_eq!(output, json!([0.0, 1.0, 2.0, 3.0])),
                    "nonretryable" => assert_eq!(
                        output,
                        json!({"native":true,"name":"NonRetryableError","message":"Workflow step is not retryable"})
                    ),
                    "hostile" => assert_eq!(
                        output,
                        json!({"observedPrivateGrant":false,"getters":0.0,"values":[7.0,{"safe":8.0}]})
                    ),
                    "lateResolve" | "lateReject" => {
                        assert_eq!(output, json!({"timeout":true}));
                        assert!(!response.drain_incomplete);
                        let (state, output, code): (String, Option<String>, String) = db
                            .lock()
                            .unwrap()
                            .query_row(
                                "SELECT state,output,code FROM steps WHERE id=?1",
                                [request.fence.instance_id.to_string()],
                                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                            )
                            .unwrap();
                        assert_eq!(state, "failed");
                        assert!(output.is_none());
                        assert_eq!(code, "WORKFLOW_STEP_TIMEOUT");
                    }
                    _ => panic!("unexpected completion"),
                }
            }
            (WorkflowOutcome::Unknown { .. }, "timeout") => {
                assert!(response.drain_incomplete);
                assert!(started.elapsed() >= Duration::from_secs(30));
                // Logical timeout is persisted, but a non-drained invocation
                // neither yields nor terminalizes the instance.
                let state: String = db
                    .lock()
                    .unwrap()
                    .query_row(
                        "SELECT state FROM runs WHERE id=?1",
                        [request.fence.instance_id.to_string()],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(state, "running");
            }
            (WorkflowOutcome::Errored { error_code, .. }, _) => {
                panic!("unexpected mode={mode}: {error_code}")
            }
            _ => panic!("unexpected mode={mode}: {:?}", response.result),
        }
    }
    assert_quarantine_and_restart(&mut harness, &db, &version, sleeping).await;
    harness.stop().await;
    server.await.unwrap();
}

async fn assert_quarantine_and_restart(
    harness: &mut Harness,
    db: &Arc<Mutex<Connection>>,
    version: &open_compute_storage::WorkflowTarget,
    sleeping: Option<WorkflowRunRequest>,
) {
    // Repeated calls after incomplete drain must not grow uncounted background
    // invocations. The transport quarantine is shared by all of its clones.
    let quarantined = envelope(&db.lock().unwrap(), "normal");
    for _ in 0..4 {
        assert!(
            harness
                .transport
                .clone()
                .dispatch_workflow(version, &quarantined, Duration::from_secs(10))
                .await
                .is_err()
        );
    }
    let count: i64 = db
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM steps WHERE id=?1",
            [quarantined.fence.instance_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 0,
        "quarantine must refuse before starting another loaded invocation"
    );
    // Keep the absolute waiting deadline while discarding the complete loaded
    // isolate. This fixture supplies the due promotion; product tests must prove
    // the same transition through the production scheduler and migrations.
    let mut sleeping = sleeping.unwrap();
    let id = sleeping.fence.instance_id.to_string();
    let due: i64 = db
        .lock()
        .unwrap()
        .query_row("SELECT deadline FROM steps WHERE id=?1", [&id], |row| {
            row.get(0)
        })
        .unwrap();
    harness.restart().await;
    {
        let db = db.lock().unwrap();
        db.execute(
            "UPDATE steps SET state='complete',output=NULL WHERE id=?1 AND state='waiting'",
            [&id],
        )
        .unwrap();
        db.execute(
            "UPDATE runs SET state='running',token=?2 WHERE id=?1",
            params![&id, "33".repeat(32)],
        )
        .unwrap();
    }
    sleeping.fence.run_token = WorkflowToken::from_bytes([0x33; 32]);
    let replay = harness
        .transport
        .dispatch_workflow(version, &sleeping, Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(replay.loader_outcome, "cold");
    assert!(
        matches!(replay.result,WorkflowOutcome::Complete{ref output_base64,..} if decode_workflow_json(output_base64)==json!("awake"))
    );
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT deadline FROM steps WHERE id=?1", [&id], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        due
    );
}
