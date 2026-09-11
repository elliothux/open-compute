use super::*;

pub(super) async fn exercise_artifacts(
    command: &WranglerCommand<'_>,
    admin_addr: SocketAddr,
    public_addr: SocketAddr,
    account: &str,
    internal_account: &str,
) {
    create_namespace(admin_addr, account).await;

    let namespaces = command
        .run(&["artifacts", "namespaces", "list", "--json"])
        .await;
    assert_success(&namespaces);
    assert!(json_contains(
        &json_stdout(&namespaces),
        "namespace",
        &Value::String("apps".into())
    ));
    let namespace = command
        .run(&["artifacts", "namespaces", "get", "apps", "--json"])
        .await;
    assert_success(&namespace);

    let created = command
        .run(&[
            "artifacts",
            "repos",
            "create",
            "site",
            "--namespace",
            "apps",
            "--description",
            "P14 Gate",
            "--default-branch",
            "main",
            "--json",
        ])
        .await;
    assert_success(&created);
    let created = json_stdout(&created);
    assert!(json_contains(
        &created,
        "name",
        &Value::String("site".into())
    ));
    assert!(json_contains(
        &created,
        "default_branch",
        &Value::String("main".into())
    ));

    for args in [
        vec![
            "artifacts",
            "repos",
            "list",
            "--namespace",
            "apps",
            "--json",
        ],
        vec![
            "artifacts",
            "repos",
            "get",
            "site",
            "--namespace",
            "apps",
            "--json",
        ],
        vec![
            "artifacts",
            "repos",
            "issue-token",
            "site",
            "--namespace",
            "apps",
            "--scope",
            "read",
            "--ttl",
            "60",
            "--json",
        ],
    ] {
        let output = command.run(&args).await;
        assert_success(&output);
    }
    exercise_worker_binding(command, public_addr, internal_account).await;

    let deleted = command
        .run(&[
            "artifacts",
            "repos",
            "delete",
            "site",
            "--namespace",
            "apps",
            "--force",
            "--json",
        ])
        .await;
    assert_success(&deleted);
}

async fn exercise_worker_binding(
    command: &WranglerCommand<'_>,
    public_addr: SocketAddr,
    internal_account: &str,
) {
    let config_path = command.project.join("wrangler.jsonc");
    let mut config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    config["artifacts"] = serde_json::json!([{"binding":"ARTIFACTS","namespace":"apps"}]);
    fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    fs::write(
        command.project.join("index.ts"),
        r#"export default { async fetch(_request, env) {
  const created = await env.ARTIFACTS.create("worker-repo", { description: "worker" });
  const repo = await env.ARTIFACTS.get(created.name);
  const token = await repo.createToken("read", 60);
  const tokens = await repo.listTokens();
  const revoked = await repo.revokeToken(token.id);
  const forked = await repo.fork("worker-fork", { defaultBranchOnly: true });
  let invalidImport = "";
  try {
    await env.ARTIFACTS.import({ source: { url: "http://127.0.0.1/repo.git" }, target: { name: "unsafe" } });
  } catch (error) { invalidImport = error.code; }
  const listed = await env.ARTIFACTS.list({ limit: 50 });
  const deleted = await env.ARTIFACTS.delete(forked.name);
  return Response.json({ created, repo: { id: repo.id, name: repo.name }, token: { id: token.id, scope: token.scope }, totalTokens: tokens.total, revoked, listed: listed.total, deleted, invalidImport });
} };"#,
    )
    .unwrap();
    let deployed = command.run(&["deploy"]).await;
    assert_success(&deployed);

    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build_http();
    let request = Request::builder()
        .uri(format!(
            "http://{public_addr}/__workers/{internal_account}/p6-wrangler-resource-gate/artifacts"
        ))
        .body(Body::empty())
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(15), client.request(request))
        .await
        .expect("Artifacts Worker binding timed out")
        .expect("Artifacts Worker binding request failed");
    assert_eq!(response.status(), 200);
    let body = to_bytes(Body::new(response.into_body()), 128 * 1024)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["created"]["name"], "worker-repo");
    assert_eq!(value["repo"]["name"], "worker-repo");
    assert_eq!(value["token"]["scope"], "read");
    assert!(value["totalTokens"].as_u64().unwrap() >= 2);
    assert_eq!(value["revoked"], true);
    assert!(value["listed"].as_u64().unwrap() >= 2);
    assert_eq!(value["deleted"], true);
    assert_eq!(value["invalidImport"], "INVALID_INPUT");
}

async fn create_namespace(admin_addr: SocketAddr, account: &str) {
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build_http();
    let request = Request::builder()
        .method("POST")
        .uri(format!(
            "http://{admin_addr}/client/v4/accounts/{account}/artifacts/namespaces"
        ))
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/json")
        .body(Body::from(r#"{"namespace":"apps"}"#))
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(3), client.request(request))
        .await
        .expect("Artifacts namespace request timed out")
        .expect("Artifacts namespace request failed");
    assert_eq!(response.status(), 200);
    let body = to_bytes(Body::new(response.into_body()), 64 * 1024)
        .await
        .unwrap();
    let envelope: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(envelope["success"], true);
}
