use super::*;

async fn bearer(application: &Router, account: &str, permissions: &[&str]) -> String {
    let response = application
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/accounts/{account}/containers/registries/registry.example.test/credentials"),
            "Bearer deployer-token",
            serde_json::to_vec(&json!({"expiration_minutes":15,"permissions":permissions}))
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let credentials = body(response).await;
    format!(
        "Bearer {}",
        credentials["result"]["password"].as_str().unwrap()
    )
}

fn assert_unauthorized(response: &Response) {
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers()[header::WWW_AUTHENTICATE],
        "Basic realm=\"Container Registry\""
    );
}

#[tokio::test]
async fn container_registry_bearer_permissions_and_ambiguous_authorization_are_fenced() {
    let fixture = fixture().await;
    let application = app(&fixture);
    let account = fixture.service.instance_id();
    let repository = format!("{account}/permissions");
    let push = bearer(&application, account.as_str(), &["push"]).await;
    let pull = bearer(&application, account.as_str(), &["pull"]).await;
    let bytes = b"permission-scoped immutable blob";
    let digest = upload(&fixture, &application, &repository, &push, bytes).await;
    let blob_path = format!("/v2/{repository}/blobs/{digest}");
    for (method, path) in [
        (Method::GET, "/v2/"),
        (Method::GET, blob_path.as_str()),
        (Method::HEAD, blob_path.as_str()),
    ] {
        assert_unauthorized(
            &application
                .clone()
                .oneshot(request(method, path, &push, Vec::new()))
                .await
                .unwrap(),
        );
    }
    assert_eq!(
        application
            .clone()
            .oneshot(request(Method::GET, "/v2/", &pull, Vec::new()))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    for method in [Method::GET, Method::HEAD] {
        let response = application
            .clone()
            .oneshot(request(method.clone(), &blob_path, &pull, Vec::new()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_LENGTH],
            bytes.len().to_string()
        );
        let fetched = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        if method == Method::GET {
            assert_eq!(fetched, bytes.as_slice());
        } else {
            assert!(fetched.is_empty());
        }
    }
    let start_path = format!("/v2/{repository}/blobs/uploads/");
    let started = application
        .clone()
        .oneshot(request(Method::POST, &start_path, &push, Vec::new()))
        .await
        .unwrap();
    assert_eq!(started.status(), StatusCode::ACCEPTED);
    let location = started.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    let pending_bytes = b"denied mutations must not claim or append this upload";
    let pending_digest = format!("sha256:{}", hex::encode(Sha256::digest(pending_bytes)));
    let finish_path = format!("{location}?digest={pending_digest}");
    let repo = ContainerAuthority::new(fixture.service.storage.db());
    let uploads_before = repo.registry_upload_ids(account).unwrap();
    let blobs_before = repo
        .registry_blobs(account, &repository)
        .unwrap()
        .into_iter()
        .map(|blob| (blob.sha256, blob.size))
        .collect::<Vec<_>>();
    for (method, path, content) in [
        (Method::POST, start_path.as_str(), Vec::new()),
        (Method::PATCH, location.as_str(), b"not appended".to_vec()),
        (Method::PUT, finish_path.as_str(), pending_bytes.to_vec()),
        (
            Method::PUT,
            &format!("/v2/{repository}/manifests/release"),
            b"{}".to_vec(),
        ),
    ] {
        assert_unauthorized(
            &application
                .clone()
                .oneshot(request(method, path, &pull, content))
                .await
                .unwrap(),
        );
        assert_eq!(repo.registry_upload_ids(account).unwrap(), uploads_before);
        assert_eq!(
            repo.registry_blobs(account, &repository)
                .unwrap()
                .into_iter()
                .map(|blob| (blob.sha256, blob.size))
                .collect::<Vec<_>>(),
            blobs_before
        );
    }
    let mut duplicate = request(Method::GET, &blob_path, &pull, Vec::new());
    duplicate
        .headers_mut()
        .append(header::AUTHORIZATION, pull.parse().unwrap());
    assert_unauthorized(&application.clone().oneshot(duplicate).await.unwrap());
    assert_eq!(repo.registry_upload_ids(account).unwrap(), uploads_before);
    assert_eq!(
        repo.registry_blobs(account, &repository)
            .unwrap()
            .into_iter()
            .map(|blob| (blob.sha256, blob.size))
            .collect::<Vec<_>>(),
        blobs_before
    );
    let finished = application
        .oneshot(request(
            Method::PUT,
            &finish_path,
            &push,
            pending_bytes.to_vec(),
        ))
        .await
        .unwrap();
    assert_eq!(finished.status(), StatusCode::CREATED);
    assert_eq!(finished.headers()["docker-content-digest"], pending_digest);
    assert!(repo.registry_upload_ids(account).unwrap().is_empty());
    assert_eq!(
        repo.registry_blobs(account, &repository).unwrap().len(),
        blobs_before.len() + 1
    );
}

fn assert_unpublished(fixture: &Fixture, repository: &str, manifest: &[u8], tag: &str) {
    let repo = ContainerAuthority::new(fixture.service.storage.db());
    let account = fixture.service.instance_id();
    let digest: [u8; 32] = Sha256::digest(manifest).into();
    assert_eq!(
        repo.image(account, &digest).unwrap_err().code(),
        ErrorCode::ResourceNotFound
    );
    assert_eq!(
        repo.registry_manifest(account, repository, &digest)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
    assert_eq!(
        repo.registry_tag(account, repository, tag)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
    let connection = rusqlite::Connection::open_with_flags(
        fixture
            .service
            .storage
            .data_dir()
            .root()
            .join("control.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let publication: (u64, u64) = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM container_registry_tags WHERE repository=?1),
                    (SELECT COUNT(*) FROM container_registry_blobs WHERE repository=?1 AND expires_at_ms=?2)",
            rusqlite::params![repository, i64::MAX],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(publication, (0, 0));
}

#[tokio::test]
async fn container_registry_manifest_failures_never_publish_partial_graph_authority() {
    let fixture = fixture().await;
    let application = app(&fixture);
    let account = fixture.service.instance_id();
    let repository = format!("{account}/graph");
    let auth = bearer(&application, account.as_str(), &["push", "pull"]).await;
    let mut tar = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_ustar();
    header.set_size(4);
    header.set_mode(0o755);
    tar.append_data(&mut header, "app", b"main".as_slice())
        .unwrap();
    let layer = tar.into_inner().unwrap();
    let layer_digest = format!("sha256:{}", hex::encode(Sha256::digest(&layer)));
    let config = serde_json::to_vec(&json!({"os":"linux","architecture":"arm64","rootfs":{"type":"layers","diff_ids":[layer_digest]}})).unwrap();
    let config_digest = upload(&fixture, &application, &repository, &auth, &config).await;
    let value = json!({"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{"mediaType":"application/vnd.oci.image.config.v1+json","digest":config_digest,"size":config.len()},"layers":[{"mediaType":"application/vnd.oci.image.layer.v1.tar","digest":layer_digest,"size":layer.len()}]});
    let manifest = serde_json::to_vec(&value).unwrap();
    let path = format!("/v2/{repository}/manifests/release");
    let missing = application
        .clone()
        .oneshot(request(Method::PUT, &path, &auth, manifest.clone()))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(body(missing).await["errors"][0]["code"], "DENIED");
    assert_unpublished(&fixture, &repository, &manifest, "release");
    assert_eq!(
        upload(&fixture, &application, &repository, &auth, &layer).await,
        layer_digest
    );
    let mut wrong_size = value;
    wrong_size["layers"][0]["size"] = json!(layer.len() + 1);
    let wrong_size = serde_json::to_vec(&wrong_size).unwrap();
    let response = application
        .clone()
        .oneshot(request(Method::PUT, &path, &auth, wrong_size.clone()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_unpublished(&fixture, &repository, &wrong_size, "release");
    assert_unpublished(&fixture, &repository, &manifest, "release");
    let wrong_digest = [0xa; 32];
    let digest_path = format!(
        "/v2/{repository}/manifests/sha256:{}",
        hex::encode(wrong_digest)
    );
    let response = application
        .clone()
        .oneshot(request(Method::PUT, &digest_path, &auth, manifest.clone()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_unpublished(&fixture, &repository, &manifest, "release");
    let repo = ContainerAuthority::new(fixture.service.storage.db());
    assert_eq!(
        repo.image(account, &wrong_digest).unwrap_err().code(),
        ErrorCode::ResourceNotFound
    );
    assert_eq!(
        repo.registry_manifest(account, &repository, &wrong_digest)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
    for method in [Method::GET, Method::HEAD] {
        let response = application
            .clone()
            .oneshot(request(
                method,
                &format!("/v2/{repository}/blobs/sha256:{}", hex::encode([0xb; 32])),
                &auth,
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    let blobs_before = repo
        .registry_blobs(account, &repository)
        .unwrap()
        .into_iter()
        .map(|blob| (blob.sha256, blob.size))
        .collect::<Vec<_>>();
    for invalid_tag in [
        ".bad".to_owned(),
        "-bad".to_owned(),
        "bad:tag".to_owned(),
        "a".repeat(129),
    ] {
        let response = application
            .clone()
            .oneshot(request(
                Method::PUT,
                &format!("/v2/{repository}/manifests/{invalid_tag}"),
                &auth,
                manifest.clone(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body(response).await["errors"][0]["code"], "DENIED");
        assert_unpublished(&fixture, &repository, &manifest, "release");
        assert_eq!(
            repo.registry_blobs(account, &repository)
                .unwrap()
                .into_iter()
                .map(|blob| (blob.sha256, blob.size))
                .collect::<Vec<_>>(),
            blobs_before
        );
    }
    let response = application
        .clone()
        .oneshot(request(Method::PUT, &path, &auth, manifest.clone()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let digest: [u8; 32] = Sha256::digest(&manifest).into();
    assert_eq!(
        repo.registry_tag(account, &repository, "release").unwrap(),
        digest
    );
    assert_eq!(
        repo.registry_manifest(account, &repository, &digest)
            .unwrap(),
        manifest
    );
    let diff_id: [u8; 32] = Sha256::digest(&layer).into();
    assert_eq!(
        repo.image(account, &digest).unwrap().diff_ids,
        vec![diff_id]
    );
    for method in [Method::GET, Method::HEAD] {
        let response = application
            .clone()
            .oneshot(request(method.clone(), &path, &auth, Vec::new()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["docker-content-digest"],
            format!("sha256:{}", hex::encode(digest))
        );
        assert_eq!(
            response.headers()[header::CONTENT_LENGTH],
            manifest.len().to_string()
        );
        let content = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        if method == Method::GET {
            assert_eq!(content, manifest.as_slice());
        } else {
            assert!(content.is_empty());
        }
    }
}

#[tokio::test]
async fn container_registry_failed_uploads_remain_fenced_after_restart_and_expire_before_recovery()
{
    use crate::runtime_bridge::WorkerdTransport;
    use bytes::Bytes;
    use open_compute_core::ResourceId;
    use std::sync::Arc;

    let fixture = fixture().await;
    let application = app(&fixture);
    let account = fixture.service.instance_id();
    let repository = format!("{account}/failed-uploads");
    let auth = bearer(&application, account.as_str(), &["push", "pull"]).await;
    let repo = ContainerAuthority::new(fixture.service.storage.db());
    let target = fixture
        .service
        .storage
        .data_dir()
        .prepare_tmp_dir()
        .unwrap()
        .join("registry-owned-target");
    std::fs::write(&target, b"unchanged owned target").unwrap();
    let mut failed = Vec::new();
    for failure in ["wrong-digest", "length", "symlink", "stream-limit"] {
        let started = application
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/{repository}/blobs/uploads/"),
                &auth,
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(started.status(), StatusCode::ACCEPTED);
        let location = started.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_owned();
        let id: ResourceId = location.rsplit('/').next().unwrap().parse().unwrap();
        let spool = fixture.service.registry_upload_path(id).unwrap();
        let mut failed_request = request(Method::PATCH, &location, &auth, b"append".to_vec());
        match failure {
            "wrong-digest" => {
                failed_request = request(
                    Method::PUT,
                    &format!("{location}?digest=sha256:{}", hex::encode([0; 32])),
                    &auth,
                    b"content does not match the claimed digest".to_vec(),
                );
            }
            "length" => std::fs::write(&spool, b"bytes outside the committed append").unwrap(),
            "symlink" => {
                std::fs::remove_file(&spool).unwrap();
                std::os::unix::fs::symlink(&target, &spool).unwrap();
            }
            "stream-limit" => {
                let limit = fixture.service.registry_request_bytes();
                assert_eq!(limit % (64 << 10), 0);
                let chunks = std::iter::repeat_n(
                    Bytes::from(vec![b'x'; 64 << 10]),
                    usize::try_from(limit / (64 << 10)).unwrap(),
                )
                .chain(std::iter::once(Bytes::from_static(b"x")))
                .map(Ok::<_, std::io::Error>);
                *failed_request.body_mut() = Body::from_stream(futures::stream::iter(chunks));
            }
            _ => unreachable!(),
        }
        let response = application.clone().oneshot(failed_request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{failure}");
        assert_eq!(body(response).await["errors"][0]["code"], "DENIED");
        assert!(
            repo.registry_blobs(account, &repository)
                .unwrap()
                .is_empty()
        );
        assert!(repo.registry_upload_ids(account).unwrap().contains(&id));
        let database = rusqlite::Connection::open_with_flags(
            fixture
                .service
                .storage
                .data_dir()
                .root()
                .join("control.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let busy: bool = database
            .query_row(
                "SELECT busy FROM container_registry_uploads WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(busy, "{failure}");
        assert_eq!(std::fs::read(&target).unwrap(), b"unchanged owned target");
        if failure == "stream-limit" {
            assert_eq!(
                std::fs::metadata(&spool).unwrap().len(),
                fixture.service.registry_request_bytes()
            );
        }
        failed.push((id, location));
    }
    let restarted = Arc::new(fixture.restart_service());
    let restarted_state = fixture
        .state
        .clone()
        .with_containers(Some(Arc::clone(&restarted)));
    let restarted_router = crate::cloudflare_v4::router(
        restarted_state.clone(),
        crate::cloudflare_v4::storage_router(),
    )
    .merge(router())
    .with_state(restarted_state);
    for (id, location) in &failed {
        let retry = restarted_router
            .clone()
            .oneshot(request(Method::PATCH, location, &auth, b"retry".to_vec()))
            .await
            .unwrap();
        assert_eq!(retry.status(), StatusCode::NOT_FOUND, "{id}");
        assert!(
            repo.registry_blobs(account, &repository)
                .unwrap()
                .is_empty()
        );
    }
    let live = ResourceId::generate();
    repo.create_registry_upload(account, live, &repository, i64::MAX)
        .unwrap();
    let live_spool = fixture.service.registry_upload_path(live).unwrap();
    std::fs::write(&live_spool, b"live staging").unwrap();
    let mut expired = repo
        .expire_registry_staging(account, open_compute_core::wall_time_ms() + 900_001)
        .unwrap();
    failed.sort_by_key(|(id, _)| id.to_string());
    expired.sort_by_key(ToString::to_string);
    assert_eq!(
        expired,
        failed.iter().map(|(id, _)| *id).collect::<Vec<_>>()
    );
    assert_eq!(repo.registry_upload_ids(account).unwrap(), vec![live]);

    let transport = WorkerdTransport::for_test_endpoint(std::path::Path::new("/tmp/missing.sock"));
    restarted.reconcile_rollouts(&transport).await.unwrap();
    for (id, _) in failed {
        assert!(!fixture.service.registry_upload_path(id).unwrap().exists());
    }
    assert_eq!(std::fs::read(&target).unwrap(), b"unchanged owned target");
    assert_eq!(std::fs::read(&live_spool).unwrap(), b"live staging");
    let fresh_auth = bearer(&restarted_router, account.as_str(), &["push", "pull"]).await;
    let recovered = b"correct upload after positive staging cleanup";
    let digest = upload(
        &fixture,
        &restarted_router,
        &repository,
        &fresh_auth,
        recovered,
    )
    .await;
    let response = restarted_router
        .oneshot(request(
            Method::GET,
            &format!("/v2/{repository}/blobs/{digest}"),
            &fresh_auth,
            Vec::new(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), 1 << 20).await.unwrap(),
        recovered.as_slice()
    );
    assert_eq!(repo.registry_upload_ids(account).unwrap(), vec![live]);
    assert_eq!(repo.registry_blobs(account, &repository).unwrap().len(), 1);
}

#[tokio::test]
async fn container_registry_credentials_reject_invalid_issuance_without_publishing_tokens() {
    let fixture = fixture().await;
    let application = app(&fixture);
    let account = fixture.service.instance_id();
    let path =
        format!("/accounts/{account}/containers/registries/registry.example.test/credentials");
    let connection = rusqlite::Connection::open_with_flags(
        fixture
            .service
            .storage
            .data_dir()
            .root()
            .join("control.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let token_count = || {
        connection
            .query_row(
                "SELECT COUNT(*) FROM container_registry_tokens WHERE repository_scope=?1",
                [account.as_str()],
                |row| row.get::<_, u64>(0),
            )
            .unwrap()
    };
    assert_eq!(token_count(), 0);
    for (expiration, permissions, host, authorization, status, code) in [
        (
            15,
            vec![],
            Some("registry.example.test"),
            "Bearer deployer-token",
            StatusCode::BAD_REQUEST,
            9_100_003,
        ),
        (
            15,
            vec!["push", "push"],
            Some("registry.example.test"),
            "Bearer deployer-token",
            StatusCode::BAD_REQUEST,
            9_100_003,
        ),
        (
            15,
            vec!["pull", "pull"],
            Some("registry.example.test"),
            "Bearer deployer-token",
            StatusCode::BAD_REQUEST,
            9_100_003,
        ),
        (
            15,
            vec!["delete"],
            Some("registry.example.test"),
            "Bearer deployer-token",
            StatusCode::BAD_REQUEST,
            9_100_003,
        ),
        (
            15,
            vec!["push", "delete"],
            Some("registry.example.test"),
            "Bearer deployer-token",
            StatusCode::BAD_REQUEST,
            9_100_003,
        ),
        (
            15,
            vec!["push", "pull", "push"],
            Some("registry.example.test"),
            "Bearer deployer-token",
            StatusCode::BAD_REQUEST,
            9_100_003,
        ),
        (
            0,
            vec!["push", "pull"],
            Some("registry.example.test"),
            "Bearer deployer-token",
            StatusCode::BAD_REQUEST,
            9_100_003,
        ),
        (
            16,
            vec!["push", "pull"],
            Some("registry.example.test"),
            "Bearer deployer-token",
            StatusCode::BAD_REQUEST,
            9_100_003,
        ),
        (
            15,
            vec!["push", "pull"],
            Some("different.example.test"),
            "Bearer deployer-token",
            StatusCode::BAD_REQUEST,
            9_100_003,
        ),
        (
            15,
            vec!["push", "pull"],
            None,
            "Bearer deployer-token",
            StatusCode::BAD_REQUEST,
            9_100_003,
        ),
        (
            15,
            vec!["push", "pull"],
            Some("registry.example.test"),
            "Bearer read-token",
            StatusCode::FORBIDDEN,
            9_100_002,
        ),
    ] {
        let mut input = request(
            Method::POST,
            &path,
            authorization,
            serde_json::to_vec(&json!({"expiration_minutes":expiration,"permissions":permissions}))
                .unwrap(),
        );
        if let Some(host) = host {
            input
                .headers_mut()
                .insert(header::HOST, host.parse().unwrap());
        } else {
            input.headers_mut().remove(header::HOST);
        }
        let response = application.clone().oneshot(input).await.unwrap();
        assert_eq!(response.status(), status);
        let error = body(response).await;
        assert_eq!(error["success"], false);
        assert_eq!(error["errors"][0]["code"], code);
        assert_eq!(token_count(), 0);
    }
    let response = application
        .clone()
        .oneshot(request(
            Method::POST,
            &path,
            "Bearer deployer-token",
            serde_json::to_vec(&json!({"expiration_minutes":1,"permissions":["pull"]})).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value = body(response).await;
    assert_eq!(value["success"], true);
    assert_eq!(value["result"]["account_id"], account.to_string());
    assert_eq!(value["result"]["username"], account.to_string());
    assert_eq!(value["result"]["registry_host"], "registry.example.test");
    let password = value["result"]["password"].as_str().unwrap();
    let auth = format!("Bearer {password}");
    assert_eq!(token_count(), 1);
    let grants: (bool, bool, i64) = connection
        .query_row(
            "SELECT can_push,can_pull,expires_at_ms FROM container_registry_tokens WHERE token_sha256=?1 AND repository_scope=?2",
            rusqlite::params![password_hash(password).as_slice(), account.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!((grants.0, grants.1), (false, true));
    assert!(grants.2 > 0);
    assert_eq!(
        application
            .clone()
            .oneshot(request(Method::GET, "/v2/", &auth, Vec::new()))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_unauthorized(
        &application
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/{account}/issuer/blobs/uploads/"),
                &auth,
                Vec::new(),
            ))
            .await
            .unwrap(),
    );
    assert_eq!(token_count(), 1);
    let both = bearer(&application, account.as_str(), &["push", "pull"]).await;
    assert_eq!(token_count(), 2);
    assert_eq!(
        application
            .clone()
            .oneshot(request(Method::GET, "/v2/", &both, Vec::new()))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}
