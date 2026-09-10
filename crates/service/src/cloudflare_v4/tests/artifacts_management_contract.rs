use super::*;
use std::path::Path;
use std::process::Command;

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

async fn request(
    state: HttpState,
    method: Method,
    path: &str,
    token: &str,
    body: Option<&str>,
) -> axum::response::Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    if body.is_some() {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    app(state)
        .oneshot(
            request
                .body(Body::from(body.unwrap_or_default().to_owned()))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn artifacts_crud_tokens_and_pagination_match_the_frozen_contract() {
    let (_temp, state, authority) = artifacts_state();
    let account = authority.public_id();
    let namespaces = format!("/accounts/{account}/artifacts/namespaces");
    let response = request(
        state.clone(),
        Method::POST,
        &namespaces,
        "deployer-token",
        Some(r#"{"namespace":"apps"}"#),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json(response).await;
    assert_eq!(body["result"]["namespace"], "apps");
    assert_eq!(body["result"]["repo_count"], 0);

    let repos = format!("{namespaces}/apps/repos");
    let created = request(
        state.clone(),
        Method::POST,
        &repos,
        "deployer-token",
        Some(r#"{"name":"alpha","description":"first"}"#),
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK);
    let created = json(created).await;
    assert_eq!(created["result"]["name"], "alpha");
    assert_eq!(created["result"]["description"], "first");
    assert_eq!(created["result"]["default_branch"], "main");
    assert!(
        created["result"]["token"]
            .as_str()
            .unwrap()
            .strip_prefix("art_v1_")
            .and_then(|value| value.split_once("?expires="))
            .is_some_and(|(secret, expiry)| {
                secret.len() == 40
                    && secret
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                    && expiry.parse::<u64>().is_ok()
            })
    );
    assert_eq!(
        created["result"]["remote"],
        "https://artifacts.example.test/git/apps/alpha.git"
    );

    let duplicate = request(
        state.clone(),
        Method::POST,
        &repos,
        "deployer-token",
        Some(r#"{"name":"alpha"}"#),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    assert_eq!(json(duplicate).await["errors"][0]["code"], 10201);

    let second = request(
        state.clone(),
        Method::POST,
        &repos,
        "deployer-token",
        Some(r#"{"name":"beta"}"#),
    )
    .await;
    assert_eq!(second.status(), StatusCode::OK);

    let first_page = request(
        state.clone(),
        Method::GET,
        &format!("{repos}?limit=1"),
        "read-token",
        None,
    )
    .await;
    let first_page = json(first_page).await;
    assert_eq!(first_page["result"].as_array().unwrap().len(), 1);
    assert_eq!(first_page["result_info"]["per_page"], 1);
    assert_eq!(first_page["result_info"]["count"], 1);
    let cursor = first_page["result_info"]["cursor"].as_str().unwrap();
    assert!(!cursor.is_empty());
    let invalid_page = request(
        state.clone(),
        Method::GET,
        &format!("{repos}?limit=1&cursor={cursor}&page=1"),
        "read-token",
        None,
    )
    .await;
    assert_eq!(invalid_page.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json(invalid_page).await["errors"][0]["code"], 10100);
    let second_page = request(
        state.clone(),
        Method::GET,
        &format!("{repos}?limit=1&cursor={cursor}"),
        "read-token",
        None,
    )
    .await;
    let second_page = json(second_page).await;
    assert_eq!(second_page["result"].as_array().unwrap().len(), 1);
    assert_eq!(second_page["result_info"]["cursor"], "");

    let tokens = format!("{repos}/alpha/tokens?state=all");
    let token_list = request(state.clone(), Method::GET, &tokens, "read-token", None).await;
    assert_eq!(token_list.status(), StatusCode::OK);
    assert_eq!(
        json(token_list).await["result"].as_array().unwrap().len(),
        1
    );

    let deleted = request(
        state.clone(),
        Method::DELETE,
        &format!("{repos}/alpha"),
        "deployer-token",
        None,
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::ACCEPTED);
    let missing = request(
        state,
        Method::GET,
        &format!("{repos}/alpha"),
        "read-token",
        None,
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(json(missing).await["errors"][0]["code"], 10200);
}

#[tokio::test]
async fn artifacts_content_fork_and_token_routes_read_real_git_objects() {
    let (temp, state, authority) = artifacts_state();
    let account = authority.public_id();
    let namespaces = format!("/accounts/{account}/artifacts/namespaces");
    let unsupported = request(
        state.clone(),
        Method::POST,
        &namespaces,
        "deployer-token",
        Some(r#"{"namespace":"placed","jurisdiction":"eu"}"#),
    )
    .await;
    assert_eq!(unsupported.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(
        request(
            state.clone(),
            Method::POST,
            &namespaces,
            "deployer-token",
            Some(r#"{"namespace":"apps"}"#),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let repos = format!("{namespaces}/apps/repos");
    for (path, body) in [
        (&repos, r#"{"name":"bad/name"}"#),
        (&repos, r#"{"name":"bad","unknown":true}"#),
    ] {
        assert_eq!(
            request(
                state.clone(),
                Method::POST,
                path,
                "deployer-token",
                Some(body),
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        request(
            state.clone(),
            Method::POST,
            &repos,
            "deployer-token",
            Some(r#"{"name":"source","description":"content"}"#),
        )
        .await
        .status(),
        StatusCode::OK
    );

    let api = state.artifact_api().unwrap();
    let repository = api
        .repository(authority.internal_id(), "apps", "source")
        .unwrap();
    let work = temp.path().join("work");
    git(
        temp.path(),
        &[
            "clone",
            api.git().path(repository.id).to_str().unwrap(),
            work.to_str().unwrap(),
        ],
    );
    git(&work, &["config", "user.name", "Open Compute Test"]);
    git(&work, &["config", "user.email", "test@open-compute.dev"]);
    std::fs::write(work.join("README.md"), b"first\n").unwrap();
    git(&work, &["add", "README.md"]);
    git(&work, &["commit", "-m", "first"]);
    std::fs::write(work.join("README.md"), b"second\n").unwrap();
    std::fs::write(work.join("app.js"), b"export default 1;\n").unwrap();
    std::fs::write(work.join("data.bin"), b"a\0b").unwrap();
    std::fs::write(work.join("index.html"), b"<p>ok</p>\n").unwrap();
    std::fs::write(work.join("style.css"), b"p{}\n").unwrap();
    std::fs::write(work.join("data.json"), b"{}\n").unwrap();
    std::fs::write(work.join("image.svg"), b"<svg/>\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-m", "second"]);
    git(&work, &["push", "origin", "main"]);
    let commit = git(&work, &["rev-parse", "HEAD"]);
    let tree = git(&work, &["rev-parse", "HEAD^{tree}"]);
    let blob = git(&work, &["rev-parse", "HEAD:README.md"]);

    for path in [
        format!("{namespaces}/apps"),
        format!("{repos}/source"),
        format!("{repos}?search=sour&sort=name&direction=asc"),
    ] {
        assert_eq!(
            request(state.clone(), Method::GET, &path, "read-token", None)
                .await
                .status(),
            StatusCode::OK
        );
    }
    for path in [
        format!("{namespaces}?limit=0"),
        format!("{repos}?sort=invalid"),
        format!("{repos}?direction=sideways"),
        format!("{repos}/source?query=forbidden"),
    ] {
        assert_eq!(
            request(state.clone(), Method::GET, &path, "read-token", None)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        request(
            state.clone(),
            Method::GET,
            &format!("{repos}/missing"),
            "read-token",
            None,
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let response = request(
        state.clone(),
        Method::GET,
        &format!("{repos}/source/blob/{blob}"),
        "read-token",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap(),
        "second\n"
    );
    for (kind, oid) in [("commit", &commit), ("tree", &tree)] {
        let response = request(
            state.clone(),
            Method::GET,
            &format!("{repos}/source/{kind}/{oid}"),
            "read-token",
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(json(response).await["result"]["type"], kind);
    }
    let wrong_kind = request(
        state.clone(),
        Method::GET,
        &format!("{repos}/source/commit/{blob}"),
        "read-token",
        None,
    )
    .await;
    assert_eq!(wrong_kind.status(), StatusCode::NOT_FOUND);

    let file = request(
        state.clone(),
        Method::GET,
        &format!("{repos}/source/file?ref=main&path=README.md"),
        "read-token",
        None,
    )
    .await;
    assert_eq!(file.status(), StatusCode::OK);
    assert_eq!(
        file.headers()[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    assert_eq!(to_bytes(file.into_body(), 1024).await.unwrap(), "second\n");
    for (path, media_type) in [
        ("README.md", "text/plain; charset=utf-8"),
        ("app.js", "text/javascript; charset=utf-8"),
        ("index.html", "text/html; charset=utf-8"),
        ("style.css", "text/css; charset=utf-8"),
        ("data.json", "application/json"),
        ("image.svg", "image/svg+xml"),
        ("data.bin", "application/octet-stream"),
    ] {
        let response = request(
            state.clone(),
            Method::GET,
            &format!("{repos}/source/raw/main/{path}"),
            "read-token",
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], media_type);
    }
    let log = request(
        state.clone(),
        Method::GET,
        &format!("{repos}/source/log?ref=main&limit=1&offset=1"),
        "read-token",
        None,
    )
    .await;
    assert_eq!(log.status(), StatusCode::OK);
    assert_eq!(json(log).await["result"].as_array().unwrap().len(), 1);
    for path in [
        format!("{repos}/source/file?ref=main"),
        format!("{repos}/source/log?limit=0"),
        format!("{repos}/source/raw/main/missing.txt"),
    ] {
        let status = request(state.clone(), Method::GET, &path, "read-token", None)
            .await
            .status();
        assert!(matches!(
            status,
            StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND
        ));
    }

    let fork = request(
        state.clone(),
        Method::POST,
        &format!("{repos}/source/fork"),
        "deployer-token",
        Some(r#"{"name":"forked","default_branch_only":false}"#),
    )
    .await;
    assert_eq!(fork.status(), StatusCode::OK);
    assert!(json(fork).await["result"]["objects"].as_u64().unwrap() >= 6);

    for body in [
        r#"{"repo":"source","scope":"invalid","ttl":60}"#,
        r#"{"repo":"source","scope":"read","ttl":59}"#,
    ] {
        assert_eq!(
            request(
                state.clone(),
                Method::POST,
                &format!("{namespaces}/apps/tokens"),
                "deployer-token",
                Some(body),
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }

    let issued = request(
        state.clone(),
        Method::POST,
        &format!("{namespaces}/apps/tokens"),
        "deployer-token",
        Some(r#"{"repo":"source","scope":"read","ttl":60}"#),
    )
    .await;
    assert_eq!(issued.status(), StatusCode::OK);
    let issued = json(issued).await;
    let token = issued["result"]["id"].as_str().unwrap();
    assert_eq!(issued["result"]["scope"], "read");
    let revoked = request(
        state.clone(),
        Method::DELETE,
        &format!("{namespaces}/apps/tokens/{token}"),
        "deployer-token",
        None,
    )
    .await;
    assert_eq!(revoked.status(), StatusCode::OK);
    let revoked_list = request(
        state.clone(),
        Method::GET,
        &format!("{repos}/source/tokens?state=revoked&per_page=10&page=1"),
        "read-token",
        None,
    )
    .await;
    assert_eq!(revoked_list.status(), StatusCode::OK);
    assert_eq!(
        json(revoked_list).await["result"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        request(
            state.clone(),
            Method::GET,
            &format!("{repos}/source/tokens?state=invalid"),
            "read-token",
            None,
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            state.clone(),
            Method::DELETE,
            &format!("{namespaces}/apps/tokens/not-an-id"),
            "deployer-token",
            None,
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            state.clone(),
            Method::POST,
            &format!("{repos}/missing/fork"),
            "deployer-token",
            Some(r#"{"name":"missing-fork"}"#),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let invalid_import = request(
        state,
        Method::POST,
        &format!("{repos}/unsafe/import"),
        "deployer-token",
        Some(r#"{"url":"http://127.0.0.1/repo.git"}"#),
    )
    .await;
    assert_eq!(invalid_import.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json(invalid_import).await["errors"][0]["code"], 10100);
}
