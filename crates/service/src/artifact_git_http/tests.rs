use crate::artifact_api::ArtifactApiState;
use crate::cloudflare_v4::accounts::AccountAuthority;
use crate::{HealthCoordinator, MetricsRegistry};
use axum::http::{Request, StatusCode, header};
use base64::Engine as _;
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{ArtifactsConfig, DataConfig, MetricsConfig};
use open_compute_core::wall_time_ms;
use open_compute_storage::{ArtifactTokenScope, PlatformStorage};
use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;

fn git(cwd: &Path, token: &str, args: &[&str]) -> Output {
    let mut command = Command::new("git");
    command.args(["-c", "protocol.version=2"]);
    if !token.is_empty() {
        command
            .arg("-c")
            .arg(format!("http.extraHeader=Authorization: Bearer {token}"));
    }
    command
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", cwd)
        .output()
        .unwrap()
}

fn must_git(cwd: &Path, token: &str, args: &[&str]) {
    let output = git(cwd, token, args);
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn storage(temp: &tempfile::TempDir) -> Arc<PlatformStorage> {
    let root = temp.path().join("data");
    Arc::new(
        PlatformStorage::bootstrap(
            &DataConfig {
                path: root.clone(),
                master_key_file: root.join("keys/master.key"),
                master_key_env: None,
                sqlite_busy_timeout_ms: 5_000,
                free_space_soft_bytes: 1,
                free_space_hard_bytes: 1,
            },
            &SystemClock,
        )
        .unwrap(),
    )
}

#[test]
fn receive_ref_and_token_syntax_fail_closed() {
    assert!(super::valid_ref_name("refs/heads/main"));
    for reference in [
        "refs/heads/.hidden",
        "refs/heads/a..b",
        "refs/heads/a@{b",
        "refs/heads/a.lock",
        "refs/heads/a\\b",
        "refs/other/main",
    ] {
        assert!(!super::valid_ref_name(reference), "{reference}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_cli_push_clone_v1_v2_and_token_fences_interoperate() {
    let temp = tempfile::tempdir().unwrap();
    let storage = storage(&temp);
    let account = storage.identity().default_account_id;
    let config = ArtifactsConfig {
        max_request_bytes: 16 * 1024 * 1024,
        max_repository_bytes: 32 * 1024 * 1024,
        ..ArtifactsConfig::default()
    };
    let api = ArtifactApiState::new(Arc::clone(&storage), config).unwrap();
    api.create_namespace(account, "apps", wall_time_ms())
        .unwrap();
    api.create_repository(
        account,
        "apps",
        crate::artifact_api::CreateRepositoryRequest {
            name: "site",
            description: "",
            default_branch: "main",
            read_only: false,
        },
        wall_time_ms(),
    )
    .unwrap();
    let write = api
        .issue_token(
            account,
            "apps",
            "site",
            ArtifactTokenScope::Write,
            None,
            wall_time_ms(),
        )
        .unwrap();

    let authority = AccountAuthority::new(
        storage.identity().platform_id,
        account,
        storage.identity().created_at_ms,
    );
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "test").unwrap());
    let state = crate::http::HttpState::for_test(HealthCoordinator::new(), metrics, false, None)
        .with_cloudflare_v4_account(authority)
        .with_platform_storage(Arc::clone(&storage))
        .with_artifact_api(api.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, crate::http::public_router(state))
            .await
            .unwrap();
    });
    let remote = format!("http://{address}/git/apps/site.git");

    let secret = write.plaintext.split_once('?').unwrap().0;
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("x:{secret}"));
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build_http();
    let response = client
        .request(
            Request::builder()
                .uri(format!("{remote}/info/refs?service=git-upload-pack"))
                .header(header::AUTHORIZATION, format!("Basic {basic}"))
                .body(http_body_util::Empty::<bytes::Bytes>::new())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let work = temp.path().join("work");
    let token = write.plaintext.clone();
    let remote_for_push = remote.clone();
    let temp_path = temp.path().to_path_buf();
    tokio::task::spawn_blocking(move || {
        must_git(
            &temp_path,
            &token,
            &["clone", &remote_for_push, work.to_str().unwrap()],
        );
        must_git(&work, "", &["config", "user.name", "Open Compute Test"]);
        must_git(
            &work,
            "",
            &["config", "user.email", "test@open-compute.dev"],
        );
        std::fs::write(work.join("README.md"), b"hello artifacts\n").unwrap();
        must_git(&work, "", &["add", "README.md"]);
        must_git(&work, "", &["commit", "-m", "first"]);
        must_git(&work, &token, &["push", "origin", "main"]);
    })
    .await
    .unwrap();

    let read = api
        .issue_token(
            account,
            "apps",
            "site",
            ArtifactTokenScope::Read,
            None,
            wall_time_ms(),
        )
        .unwrap();
    let clone_v2 = temp.path().join("clone-v2");
    let clone_v1 = temp.path().join("clone-v1");
    let remote_for_clone = remote.clone();
    let read_plaintext = read.plaintext.clone();
    let temp_path = temp.path().to_path_buf();
    tokio::task::spawn_blocking(move || {
        must_git(
            &temp_path,
            &read_plaintext,
            &["clone", &remote_for_clone, clone_v2.to_str().unwrap()],
        );
        let output = Command::new("git")
            .args(["-c", "protocol.version=1"])
            .arg("-c")
            .arg(format!(
                "http.extraHeader=Authorization: Bearer {read_plaintext}"
            ))
            .args(["clone", &remote_for_clone, clone_v1.to_str().unwrap()])
            .current_dir(&temp_path)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &temp_path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read(clone_v1.join("README.md")).unwrap(),
            b"hello artifacts\n"
        );
        must_git(&clone_v2, "", &["config", "user.name", "Open Compute Test"]);
        must_git(
            &clone_v2,
            "",
            &["config", "user.email", "test@open-compute.dev"],
        );
        std::fs::write(clone_v2.join("blocked.txt"), b"blocked\n").unwrap();
        must_git(&clone_v2, "", &["add", "blocked.txt"]);
        must_git(&clone_v2, "", &["commit", "-m", "blocked"]);
        assert!(
            !git(&clone_v2, &read_plaintext, &["push", "origin", "main"])
                .status
                .success()
        );
    })
    .await
    .unwrap();

    api.revoke_token_value(
        account,
        "apps",
        "site",
        &read.record.id.to_string(),
        wall_time_ms(),
    )
    .unwrap();
    let rejected = temp.path().join("rejected");
    let remote_for_reject = remote;
    let temp_path = temp.path().to_path_buf();
    let revoked = read.plaintext;
    let output = tokio::task::spawn_blocking(move || {
        git(
            &temp_path,
            &revoked,
            &["clone", &remote_for_reject, rejected.to_str().unwrap()],
        )
    })
    .await
    .unwrap();
    assert!(!output.status.success());
    server.abort();
}
