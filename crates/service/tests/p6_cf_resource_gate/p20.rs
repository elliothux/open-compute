use super::*;
use serde_json::json;

pub(super) async fn exercise_p20_project_workflow(fixture: &Fixture) {
    let root = fixture.project.parent().unwrap().join("p20-projects");
    fs::create_dir(&root).unwrap();
    let config_home = root.join("client-config");
    fs::create_dir_all(config_home.join("user")).unwrap();
    for path in [&config_home, &config_home.join("user")] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let token_file = root.join("deployer.token");
    write_token(&token_file, TOKEN);
    let api_base_url = format!("http://{}/client/v4", fixture.admin_addr);
    assert_success(
        &run_ocd(
            &config_home,
            &[
                "target",
                "add",
                "live",
                "--api-base-url",
                &api_base_url,
                "--instance-id",
                &fixture.public_account,
                "--token-file",
                token_file.to_str().unwrap(),
            ],
            None,
        )
        .await,
    );
    assert!(config_home.join("user/targets.toml").is_file());
    let daemon_pid = fixture.process.0.id();
    let alpha = create_project(&root, "alpha");
    let beta = create_project(&root, "beta");
    let framework = create_project(&root, "framework");
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build_http();
    for (project, name, mode, prebuilt) in [
        (&alpha, "alpha", "dev", false),
        (&beta, "beta", "staging", false),
        (&framework, "framework", "generated", true),
    ] {
        if prebuilt {
            let builder = CfCommand {
                executable: fixed_cf(),
                project,
                api_base_url: api_base_url.clone(),
                account_id: &fixture.public_account,
            };
            let output = builder
                .command(&["build", "--mode", mode])
                .env_remove("CLOUDFLARE_API_TOKEN")
                .env_remove("CLOUDFLARE_ACCOUNT_ID")
                .env_remove("CLOUDFLARE_API_BASE_URL")
                .output()
                .await
                .unwrap();
            assert_success(&output);
        }
        let mut args = vec![
            "cf",
            "--target",
            "live",
            "--project",
            project.to_str().unwrap(),
            "deploy",
            "--mode",
            mode,
        ];
        if prebuilt {
            args.push("--prebuilt");
        }
        let output = run_ocd(&config_home, &args, Some(project)).await;
        assert_success(&output);
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("CF_TARGET kind=target name=live")
        );
        assert_eq!(fixture.process.0.id(), daemon_pid, "deploy restarted ocd");
        let script = format!("p20-wrapper-{name}-{mode}");
        assert_project(&client, fixture, &script, name, mode).await;
        assert_deployment(&client, fixture, &script).await;
    }
    let secret_file = alpha.join("secret.json");
    fs::write(
        &secret_file,
        json!({"name":"P20_SECRET", "text":TAIL_SECRET, "type":"secret_text"}).to_string(),
    )
    .unwrap();
    fs::set_permissions(&secret_file, fs::Permissions::from_mode(0o600)).unwrap();
    let body = format!("@{}", secret_file.display());
    let prefix = [
        "cf",
        "--target",
        "live",
        "--project",
        alpha.to_str().unwrap(),
    ];
    let mut args = prefix.to_vec();
    args.extend([
        "workers",
        "secrets",
        "update",
        "P20_SECRET",
        "--worker",
        "p20-wrapper-alpha-dev",
        "--body",
        &body,
    ]);
    assert_success(&run_ocd(&config_home, &args, Some(&alpha)).await);
    args = prefix.to_vec();
    args.extend([
        "workers",
        "secrets",
        "list",
        "--worker",
        "p20-wrapper-alpha-dev",
    ]);
    let output = run_ocd(&config_home, &args, Some(&alpha)).await;
    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("P20_SECRET"));
    args = prefix.to_vec();
    args.extend([
        "workers",
        "secrets",
        "list",
        "--worker",
        "p20-wrapper-beta-staging",
    ]);
    let output = run_ocd(&config_home, &args, Some(&beta)).await;
    assert_success(&output);
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("P20_SECRET"),
        "secret crossed project and mode authority"
    );
    fs::write(
        &secret_file,
        json!({"secrets":{
            "P20_SECRET": null,
            "P20_KEEP": {"name":"P20_KEEP", "type":"secret_text", "text":TAIL_SECRET}
        }})
        .to_string(),
    )
    .unwrap();
    args = prefix.to_vec();
    args.extend([
        "workers",
        "secrets",
        "bulk",
        "--worker",
        "p20-wrapper-alpha-dev",
        "--body",
        &body,
    ]);
    assert_success(&run_ocd(&config_home, &args, Some(&alpha)).await);
    args = prefix.to_vec();
    args.extend(["deploy", "--mode", "dev"]);
    assert_success(&run_ocd(&config_home, &args, Some(&alpha)).await);
    args = prefix.to_vec();
    args.extend([
        "workers",
        "secrets",
        "list",
        "--worker",
        "p20-wrapper-alpha-dev",
    ]);
    let output = run_ocd(&config_home, &args, Some(&alpha)).await;
    assert_success(&output);
    let secrets = String::from_utf8_lossy(&output.stdout);
    assert!(secrets.contains("P20_KEEP"));
    assert!(!secrets.contains("P20_SECRET"));
    args = prefix.to_vec();
    args.extend([
        "workers",
        "secrets",
        "delete",
        "P20_KEEP",
        "--worker",
        "p20-wrapper-alpha-dev",
        "--force",
    ]);
    assert_success(&run_ocd(&config_home, &args, Some(&alpha)).await);
    args = prefix.to_vec();
    args.extend([
        "workers",
        "secrets",
        "list",
        "--worker",
        "p20-wrapper-alpha-dev",
    ]);
    let output = run_ocd(&config_home, &args, Some(&alpha)).await;
    assert_success(&output);
    assert!(!String::from_utf8_lossy(&output.stdout).contains("P20_KEEP"));

    // A recorded Build Output account must not redirect the selected credentials.
    let output_config = framework.join(".cloudflare/output/v0/config.json");
    let original = fs::read(&output_config).unwrap();
    let mut output: Value = serde_json::from_slice(&original).unwrap();
    output["accountId"] = json!("00000000000000000000000000000000");
    fs::write(&output_config, output.to_string()).unwrap();
    let output = run_ocd(
        &config_home,
        &[
            "cf",
            "--target",
            "live",
            "--project",
            framework.to_str().unwrap(),
            "deploy",
            "--prebuilt",
            "--mode",
            "generated",
        ],
        Some(&framework),
    )
    .await;
    assert!(
        !output.status.success(),
        "prebuilt account mismatch was accepted"
    );
    assert_clean_output(&output.stdout);
    assert_clean_output(&output.stderr);
    fs::write(&output_config, original).unwrap();
    assert_project(
        &client,
        fixture,
        "p20-wrapper-framework-generated",
        "framework",
        "generated",
    )
    .await;
    assert_deployment(&client, fixture, "p20-wrapper-framework-generated").await;
    reject_unavailable_bindings(&client, fixture).await;
    args = prefix.to_vec();
    args.extend(["kv", "namespaces", "list"]);
    assert_success(&run_ocd(&config_home, &args, Some(&alpha)).await);
    args = prefix.to_vec();
    args.extend([
        "workers",
        "delete",
        "p20-wrapper-framework-generated",
        "--force",
        "--delete-with-references",
        "true",
    ]);
    assert_success(&run_ocd(&config_home, &args, Some(&framework)).await);
    args = prefix.to_vec();
    args.extend(["workers", "get", "p20-wrapper-framework-generated"]);
    let output = run_ocd(&config_home, &args, Some(&framework)).await;
    assert!(
        !output.status.success(),
        "deleted Worker remained reachable through cf"
    );
    assert_clean_output(&output.stdout);
    assert_clean_output(&output.stderr);
    assert_eq!(fixture.process.0.id(), daemon_pid);
}

fn create_project(root: &Path, name: &str) -> PathBuf {
    let directory = root.join(name);
    fs::create_dir(&directory).unwrap();
    write_project_manifest(&directory);
    std::os::unix::fs::symlink(
        repo_root().join("node_modules"),
        directory.join("node_modules"),
    )
    .unwrap();
    fs::write(directory.join("vite.config.ts"), "import {cloudflare} from '@cloudflare/vite-plugin'; export default {plugins:[cloudflare({types:{generate:false}})]};").unwrap();
    fs::write(directory.join("cloudflare.config.ts"), format!("import {{bindings,defineConfig}} from 'cf/config'; export default defineConfig(({{mode}}) => ({{worker:{{name:'p20-wrapper-{name}-'+mode,entrypoint:'index.ts',compatibilityDate:'2026-09-08',workersDev:false,env:{{ENVIRONMENT:bindings.text(mode)}}}}}}));")).unwrap();
    fs::write(directory.join("index.ts"), format!("export default {{fetch(_request,env) {{return Response.json({{project:'{name}',environment:env.ENVIRONMENT}});}}}};")).unwrap();
    fs::write(directory.join(".env"), "CLOUDFLARE_API_BASE_URL=http://127.0.0.1:9/client/v4\nCLOUDFLARE_ACCOUNT_ID=00000000000000000000000000000000\nCLOUDFLARE_API_TOKEN=unselected-dotenv-token\n").unwrap();
    directory
}

async fn run_ocd(config_home: &Path, arguments: &[&str], project: Option<&Path>) -> Output {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_ocd"));
    command
        .arg("--no-update-check")
        .args(arguments)
        .env("XDG_CONFIG_HOME", config_home)
        .env("OPEN_COMPUTE_TEST_OCD_ROOT", config_home)
        .env("CF_SEND_TELEMETRY", "false")
        .env("DO_NOT_TRACK", "1")
        .env("CI", "true")
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("ALL_PROXY", "http://127.0.0.1:9")
        .env("all_proxy", "http://127.0.0.1:9")
        .env("NO_PROXY", "127.0.0.1,localhost,::1")
        .env("no_proxy", "127.0.0.1,localhost,::1")
        .env("CLOUDFLARE_API_TOKEN", "unselected-process-token")
        .env("CLOUDFLARE_ACCOUNT_ID", "00000000000000000000000000000000")
        .env("CLOUDFLARE_API_BASE_URL", "http://127.0.0.1:9/client/v4");
    if let Some(project) = project {
        command.current_dir(project);
    }
    tokio::time::timeout(Duration::from_secs(120), command.output())
        .await
        .expect("ocd cf command timed out")
        .expect("run ocd cf command")
}

async fn assert_project(
    client: &platform_process::Client,
    fixture: &Fixture,
    script: &str,
    project: &str,
    environment: &str,
) {
    let request = Request::builder()
        .uri(format!("http://{}/probe", fixture.public_addr))
        .header("host", worker_host(&fixture.internal_account, script))
        .body(Body::empty())
        .unwrap();
    let response = client.request(request).await.unwrap();
    assert_eq!(response.status(), 200);
    let body = to_bytes(Body::new(response.into_body()), 64 * 1024)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"project": project, "environment": environment})
    );
}

async fn assert_deployment(client: &platform_process::Client, fixture: &Fixture, script: &str) {
    let request = Request::builder()
        .uri(format!(
            "http://{}/client/v4/accounts/{}/workers/scripts/{script}/deployments",
            fixture.admin_addr, fixture.public_account
        ))
        .header("authorization", format!("Bearer {READ_ONLY_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let response = client.request(request).await.unwrap();
    assert_eq!(response.status(), 200);
    let body = to_bytes(Body::new(response.into_body()), 64 * 1024)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    let deployments = value["result"]["deployments"].as_array().unwrap();
    assert_eq!(deployments.len(), 1);
    assert_eq!(deployments[0]["versions"].as_array().unwrap().len(), 1);
    assert_eq!(deployments[0]["versions"][0]["percentage"], 100);
}

async fn reject_unavailable_bindings(client: &platform_process::Client, fixture: &Fixture) {
    for (kind, status) in [
        ("analytics_engine", 400),
        ("browser", 500), // This fixture has no operator-configured Browser backend.
        ("hyperdrive", 400),
        ("mtls_certificate", 400),
        ("ratelimit", 400),
        ("dispatch_namespace", 400),
    ] {
        let metadata = json!({"main_module":"index.js", "compatibility_date":"2026-09-08", "bindings":[{"type":kind,"name":"UNSUPPORTED"}]});
        let body = format!(
            "--p20-boundary\r\nContent-Disposition: form-data; name=\"metadata\"\r\nContent-Type: application/json\r\n\r\n{metadata}\r\n--p20-boundary\r\nContent-Disposition: form-data; name=\"index.js\"; filename=\"index.js\"\r\nContent-Type: application/javascript+module\r\n\r\nexport default {{fetch(){{return new Response('wrong')}}}};\r\n--p20-boundary--\r\n"
        );
        let request = Request::builder()
            .method("PUT")
            .uri(format!(
                "http://{}/client/v4/accounts/{}/workers/scripts/p20-wrapper-beta-staging",
                fixture.admin_addr, fixture.public_account
            ))
            .header("authorization", format!("Bearer {TOKEN}"))
            .header("content-type", "multipart/form-data; boundary=p20-boundary")
            .body(Body::from(body))
            .unwrap();
        let response = client.request(request).await.unwrap();
        assert_eq!(
            response.status(),
            status,
            "unavailable binding {kind} was admitted"
        );
        let bytes = to_bytes(Body::new(response.into_body()), 64 * 1024)
            .await
            .unwrap();
        assert_clean_output(&bytes);
        assert_project(
            client,
            fixture,
            "p20-wrapper-beta-staging",
            "beta",
            "staging",
        )
        .await;
        assert_deployment(client, fixture, "p20-wrapper-beta-staging").await;
    }
}
