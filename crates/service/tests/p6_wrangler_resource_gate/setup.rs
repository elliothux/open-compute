use super::*;

pub(super) async fn discover_public_account(
    client: &platform_process::Client,
    admin_addr: SocketAddr,
) -> String {
    let request = Request::builder()
        .uri(format!("http://{admin_addr}/client/v4/accounts"))
        .header("authorization", format!("Bearer {READ_ONLY_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(3), client.request(request))
        .await
        .expect("public account discovery timed out")
        .expect("public account discovery request failed");
    assert_eq!(response.status(), 200);
    let body = to_bytes(Body::new(response.into_body()), 64 * 1024)
        .await
        .unwrap();
    let envelope: Value = serde_json::from_slice(&body).unwrap();
    let accounts = envelope["result"].as_array().unwrap();
    assert_eq!(accounts.len(), 1);
    let public_account = accounts[0]["id"].as_str().unwrap();
    assert!(
        public_account.len() == 32 && public_account.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "v4 account discovery must return one stable 32-hex public ID"
    );
    public_account.to_owned()
}

pub(super) async fn wait_ready(
    client: &platform_process::Client,
    admin_addr: SocketAddr,
    process: &mut platform_process::Process,
    log: &Path,
) {
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if process.0.try_wait().unwrap().is_some() {
            let log = fs::read(log).unwrap_or_default();
            assert_clean_output(&log);
            panic!(
                "ocd exited before readiness: {}",
                String::from_utf8_lossy(&log)
            );
        }
        if platform_process::response(client, admin_addr, "/health/ready", "GET")
            .await
            .is_ok_and(|response| response.status() == 200)
        {
            return;
        }
        if Instant::now() >= deadline {
            let log = fs::read(log).unwrap_or_default();
            assert_clean_output(&log);
            panic!(
                "ocd readiness timed out; retained sanitized failure evidence; stderr={}",
                String::from_utf8_lossy(&log)
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub(super) fn append_resource_config(
    config: &Path,
    root: &Path,
    embedding_base_url: &str,
    admin_addr: SocketAddr,
) {
    // platform_process::config already declares the three auth tables; only replace
    // the secret file contents so this Gate can assert its own token values.
    write_token(&root.join("deployer.token"), TOKEN);
    write_token(&root.join("read-only.token"), READ_ONLY_TOKEN);
    let mut file = fs::OpenOptions::new().append(true).open(config).unwrap();
    writeln!(
        file,
        r#"
[observability]
external_control_origin = "http://{}"

{}"#,
        admin_addr,
        search::ai_config_toml(embedding_base_url),
    )
    .unwrap();
}

pub(super) fn write_token(path: &Path, value: &str) {
    fs::write(path, format!("{value}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

pub(super) fn seed_workflow(storage: &PlatformStorage) {
    let account = storage.identity().default_account_id;
    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(
            account,
            "resource-gate-worker",
            RequestId::generate(),
            1,
            1_000_000,
        )
        .unwrap();
    let worker_version = VersionId::generate();
    workers
        .insert_staging_version(
            &NewVersion {
                id: worker_version,
                account_id: account,
                worker_id: worker.id,
                content_kind: VersionContentKind::Worker,
                artifact_sha256: Some([1; 32]),
                artifact_size: Some(1),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".into()),
                worker_code_sha256: [2; 32],
                compatibility_date: "2026-09-08".into(),
                compatibility_flags: Vec::new(),
                vars: Default::default(),
                secrets: Default::default(),
                request_id: RequestId::generate(),
                now_ms: 2,
            },
            &NewVersionProducts::default(),
            1_000_000,
        )
        .unwrap();
    workers.begin_validation(worker_version).unwrap();
    workers.mark_ready(worker_version, 3).unwrap();
    workers
        .promote(
            account,
            worker.id,
            worker_version,
            None,
            RequestId::generate(),
            4,
        )
        .unwrap();
    let workflows = WorkflowRepository::new(storage.db());
    let definition = workflows
        .create_definition(account, WORKFLOW_NAME, 5)
        .unwrap();
    let version = workflows
        .stage_version(account, definition.id, worker_version, "ResourceFlow", 6)
        .unwrap();
    workflows
        .finish_version(account, version.target.workflow_version_id, true, 7)
        .unwrap();
}

pub(super) fn write_config(
    project: &Path,
    account_id: &str,
    kv_id: Option<&str>,
    d1_id: Option<&str>,
) {
    let schema = fixed_wrangler()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("config-schema.json");
    let mut config = serde_json::json!({
        "$schema": schema,
        "name": "p6-wrangler-resource-gate",
        "main": "index.ts",
        "account_id": account_id,
        "compatibility_date": "2026-09-08",
        "workers_dev": false,
        "send_metrics": false,
    });
    if let Some(id) = kv_id {
        config["kv_namespaces"] = serde_json::json!([{"binding":"KV", "id":id}]);
    }
    if let Some(id) = d1_id {
        config["d1_databases"] = serde_json::json!([{
            "binding":"DB", "database_name":D1_NAME, "database_id":id,
            "migrations_dir":"migrations"
        }]);
    }
    fs::write(
        project.join("wrangler.jsonc"),
        serde_json::to_vec_pretty(&config).unwrap(),
    )
    .unwrap();
}
