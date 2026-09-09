use super::*;
use axum::body::{Body, to_bytes};
use axum::http::Request;
use rustix::process::{Pid, Signal, kill_process};
use serde_json::json;
use std::os::unix::fs::symlink;

pub(super) async fn exercise_p12_project_workflow(fixture: &Fixture) {
    let root = fixture.project.parent().unwrap().join("p12-projects");
    fs::create_dir(&root).unwrap();
    let config_home = root.join("client-config");
    fs::create_dir(&config_home).unwrap();
    fs::set_permissions(&config_home, fs::Permissions::from_mode(0o700)).unwrap();
    let token_file = root.join("deployer.token");
    fs::write(&token_file, format!("{TOKEN}\n")).unwrap();
    fs::set_permissions(&token_file, fs::Permissions::from_mode(0o600)).unwrap();
    let api_base_url = format!("http://{}/client/v4", fixture.admin_addr);

    let add = run_ocd(
        &config_home,
        &[
            "target",
            "add",
            "live",
            "--api-base-url",
            &api_base_url,
            "--account-id",
            &fixture.public_account,
            "--token-file",
            token_file.to_str().unwrap(),
        ],
        None,
    )
    .await;
    assert_success(&add);
    assert_clean_output(&add.stdout);
    assert_clean_output(&add.stderr);

    let alpha = create_project(&root, "alpha", "dev", &fixture.public_account, false);
    let beta = create_project(&root, "beta", "staging", &fixture.public_account, false);
    let framework = create_project(
        &root,
        "framework",
        "generated",
        &fixture.public_account,
        true,
    );
    let daemon_pid = fixture.process.0.id();
    for (project, arguments) in [
        (
            &alpha,
            vec!["deploy", "--env", "dev", "--config", "wrangler.jsonc"],
        ),
        (&beta, vec!["deploy", "--env", "staging"]),
        (&framework, vec!["deploy"]),
    ] {
        let project_text = project.to_str().unwrap();
        let mut wrapper_arguments = vec!["wrangler", "--target", "live", "--project", project_text];
        wrapper_arguments.extend(arguments);
        let output = run_ocd(&config_home, &wrapper_arguments, Some(project)).await;
        assert_success(&output);
        assert_clean_output(&output.stdout);
        assert_clean_output(&output.stderr);
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("WRANGLER_TARGET kind=target name=live")
        );
        assert_eq!(fixture.process.0.id(), daemon_pid, "deploy restarted ocd");
    }

    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build_http();
    assert_project(&client, fixture, "p12-wrapper-alpha-dev", "alpha", "dev").await;
    assert_project(
        &client,
        fixture,
        "p12-wrapper-beta-staging",
        "beta",
        "staging",
    )
    .await;
    assert_project(
        &client,
        fixture,
        "p12-wrapper-framework-generated",
        "framework",
        "generated",
    )
    .await;
    assert_deployment(&client, fixture, "p12-wrapper-alpha-dev").await;
    assert_deployment(&client, fixture, "p12-wrapper-beta-staging").await;
    assert_deployment(&client, fixture, "p12-wrapper-framework-generated").await;

    let secret = run_ocd_with_input(
        &config_home,
        &[
            "wrangler",
            "--target",
            "live",
            "--project",
            alpha.to_str().unwrap(),
            "secret",
            "put",
            "P12_SECRET",
            "--env",
            "dev",
        ],
        &format!("{TAIL_SECRET}\n"),
        &alpha,
    )
    .await;
    assert_success(&secret);
    assert_clean_output(&secret.stdout);
    assert_clean_output(&secret.stderr);
    let secrets = run_ocd(
        &config_home,
        &[
            "wrangler",
            "--target",
            "live",
            "--project",
            alpha.to_str().unwrap(),
            "secret",
            "list",
            "--env",
            "dev",
        ],
        Some(&alpha),
    )
    .await;
    assert_success(&secrets);
    assert!(String::from_utf8_lossy(&secrets.stdout).contains("P12_SECRET"));
    assert_clean_output(&secrets.stdout);
    assert_clean_output(&secrets.stderr);
    let beta_secrets = run_ocd(
        &config_home,
        &[
            "wrangler",
            "--target",
            "live",
            "--project",
            beta.to_str().unwrap(),
            "secret",
            "list",
            "--env",
            "staging",
        ],
        Some(&beta),
    )
    .await;
    assert_success(&beta_secrets);
    assert!(
        !String::from_utf8_lossy(&beta_secrets.stdout).contains("P12_SECRET"),
        "secret crossed project and environment authority"
    );
    assert_clean_output(&beta_secrets.stdout);
    assert_clean_output(&beta_secrets.stderr);
    let resources = run_ocd(
        &config_home,
        &[
            "wrangler",
            "--target",
            "live",
            "--project",
            alpha.to_str().unwrap(),
            "kv",
            "namespace",
            "list",
            "--config",
            "wrangler.jsonc",
        ],
        Some(&alpha),
    )
    .await;
    assert_success(&resources);
    assert_clean_output(&resources.stdout);
    assert_clean_output(&resources.stderr);

    exercise_wrapper_tail(&config_home, &alpha, &client, fixture).await;
    assert_eq!(fixture.process.0.id(), daemon_pid, "tail restarted ocd");
}

fn create_project(
    root: &Path,
    project: &str,
    environment: &str,
    account: &str,
    generated: bool,
) -> PathBuf {
    let directory = root.join(project);
    fs::create_dir(&directory).unwrap();
    let bin = directory.join("node_modules/.bin");
    fs::create_dir_all(&bin).unwrap();
    symlink(fixed_wrangler(), bin.join("wrangler")).unwrap();
    fs::write(
        directory.join("index.ts"),
        format!(
            "export default {{ fetch(_request, env) {{ console.log('p12-tail-{project}'); return Response.json({{ project: '{project}', environment: env.ENVIRONMENT }}); }} }};"
        ),
    )
    .unwrap();
    let script = format!("p12-wrapper-{project}-{environment}");
    let schema = fixed_wrangler()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("config-schema.json");
    let (config_path, config) = if generated {
        let deploy = directory.join(".wrangler/deploy");
        let dist = directory.join("dist");
        fs::create_dir_all(&deploy).unwrap();
        fs::create_dir(&dist).unwrap();
        fs::write(
            deploy.join("config.json"),
            serde_json::to_vec_pretty(&json!({"configPath": "../../dist/wrangler.jsonc"})).unwrap(),
        )
        .unwrap();
        (
            dist.join("wrangler.jsonc"),
            json!({
                "$schema": schema,
                "name": script,
                "main": "../index.ts",
                "account_id": account,
                "compatibility_date": "2026-09-08",
                "workers_dev": false,
                "send_metrics": false,
                "vars": {"ENVIRONMENT": environment}
            }),
        )
    } else {
        let mut environments = serde_json::Map::new();
        environments.insert(
            environment.to_owned(),
            json!({
                "name": script,
                "workers_dev": false,
                "vars": {"ENVIRONMENT": environment}
            }),
        );
        (
            directory.join("wrangler.jsonc"),
            json!({
                "$schema": schema,
                "name": format!("p12-wrapper-{project}"),
                "main": "index.ts",
                "account_id": account,
                "compatibility_date": "2026-09-08",
                "workers_dev": false,
                "send_metrics": false,
                "env": environments
            }),
        )
    };
    fs::write(config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    directory
}

async fn run_ocd(config_home: &Path, arguments: &[&str], project: Option<&Path>) -> Output {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_ocd"));
    command
        .arg("--no-update-check")
        .args(arguments)
        .env("XDG_CONFIG_HOME", config_home)
        .env("WRANGLER_NO_SKILLS_UPDATE_PROMPTS", "true")
        .env("WRANGLER_HIDE_BANNER", "true")
        .env("DO_NOT_TRACK", "1")
        .env("CI", "true")
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env_remove("CLOUDFLARE_API_TOKEN")
        .env_remove("CLOUDFLARE_ACCOUNT_ID")
        .env_remove("CLOUDFLARE_API_BASE_URL");
    if let Some(project) = project {
        command.current_dir(project);
    }
    tokio::time::timeout(Duration::from_secs(60), command.output())
        .await
        .expect("ocd Wrangler command timed out")
        .expect("run ocd Wrangler command")
}

async fn run_ocd_with_input(
    config_home: &Path,
    arguments: &[&str],
    input: &str,
    project: &Path,
) -> Output {
    use tokio::io::AsyncWriteExt as _;

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_ocd"));
    command
        .arg("--no-update-check")
        .args(arguments)
        .current_dir(project)
        .env("XDG_CONFIG_HOME", config_home)
        .env("WRANGLER_NO_SKILLS_UPDATE_PROMPTS", "true")
        .env("WRANGLER_HIDE_BANNER", "true")
        .env("DO_NOT_TRACK", "1")
        .env("CI", "true")
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())
        .await
        .expect("ocd Wrangler command timed out")
        .expect("run ocd Wrangler command")
}

async fn assert_project(
    client: &platform_process::Client,
    fixture: &Fixture,
    script: &str,
    project: &str,
    environment: &str,
) {
    let request = Request::builder()
        .uri(format!(
            "http://{}/__workers/{}/{script}/probe",
            fixture.public_addr, fixture.internal_account
        ))
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

async fn exercise_wrapper_tail(
    config_home: &Path,
    project: &Path,
    client: &platform_process::Client,
    fixture: &Fixture,
) {
    let stdout = project.join("tail.stdout");
    let stderr = project.join("tail.stderr");
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_ocd"));
    command
        .args([
            "--no-update-check",
            "wrangler",
            "--target",
            "live",
            "--project",
            project.to_str().unwrap(),
            "tail",
            "--env",
            "dev",
            "--format=json",
        ])
        .env("XDG_CONFIG_HOME", config_home)
        .env("WRANGLER_NO_SKILLS_UPDATE_PROMPTS", "true")
        .env("WRANGLER_HIDE_BANNER", "true")
        .env("DO_NOT_TRACK", "1")
        .env("CI", "true")
        .stdout(Stdio::from(fs::File::create(&stdout).unwrap()))
        .stderr(Stdio::from(fs::File::create(&stderr).unwrap()));
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while target_tail_count(client, fixture, "p12-wrapper-alpha-dev").await != 1 {
        assert!(
            Instant::now() < deadline,
            "wrapper tail session did not start"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_project(client, fixture, "p12-wrapper-alpha-dev", "alpha", "dev").await;
    let event_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let bytes = fs::read(&stdout).unwrap_or_default();
        if String::from_utf8_lossy(&bytes).contains("p12-tail-alpha") {
            break;
        }
        assert!(
            Instant::now() < event_deadline,
            "wrapper tail did not receive the event"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let pid = Pid::from_raw(child.id().unwrap() as i32).unwrap();
    kill_process(pid, Signal::INT).unwrap();
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success(), "wrapper tail SIGINT failed: {status}");
    assert_clean_output(&fs::read(&stdout).unwrap_or_default());
    assert_clean_output(&fs::read(&stderr).unwrap_or_default());
    let cleanup_deadline = Instant::now() + Duration::from_secs(3);
    while target_tail_count(client, fixture, "p12-wrapper-alpha-dev").await != 0 {
        assert!(
            Instant::now() < cleanup_deadline,
            "wrapper tail session leaked"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn target_tail_count(
    client: &platform_process::Client,
    fixture: &Fixture,
    script: &str,
) -> usize {
    let request = Request::builder()
        .uri(format!(
            "http://{}/client/v4/accounts/{}/workers/scripts/{script}/tails",
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
    value["result"].as_array().unwrap().len()
}
