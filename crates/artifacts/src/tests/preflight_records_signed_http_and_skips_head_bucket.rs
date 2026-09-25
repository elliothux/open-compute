use super::*;

#[tokio::test]
async fn preflight_records_signed_http_and_skips_head_bucket() {
    let mock = MockS3::spawn("open-compute").await;
    let client = client_for(&mock).await;
    let out = preflight_object_storage(&client, InstanceId::generate(), StartupId::generate())
        .await
        .expect("preflight");
    assert_eq!(out.payload_bytes(), 32);
    assert_eq!(out.puts(), 1);
    assert_eq!(out.heads(), 2);
    assert_eq!(out.gets(), 1);
    assert_eq!(out.deletes(), 1);
    assert!(format!("{out:?}").contains("payload_bytes"));
    let canary = crate::PreflightOutcome::successful_canary();
    assert_eq!(canary, out);
    let rec = mock.recorded();
    assert!(rec.iter().all(|r| r.method != "HEAD"
        || r.path.contains("/preflight/")
        || r.path.contains("/artifacts/")
        || r.path.contains("/authority/")));
    assert!(
        !rec.iter().any(
            |r| r.method == "HEAD" && (r.path == "/open-compute" || r.path == "/open-compute/")
        )
    );
    let payload_ops: Vec<_> = rec
        .iter()
        .filter(|r| matches!(r.method.as_str(), "PUT" | "HEAD" | "GET" | "DELETE"))
        .collect();
    assert!(payload_ops.len() >= 5);
    assert!(payload_ops.iter().any(|r| r.method == "PUT"));
    assert!(payload_ops.iter().any(|r| r.method == "GET"));
    assert!(payload_ops.iter().all(|r| r.has_authorization));
    assert!(payload_ops.iter().all(|r| {
        r.authorization
            .as_deref()
            .is_some_and(|v| v.starts_with("AWS4-HMAC-SHA256 Credential="))
    }));
    assert_eq!(payload_ops[0].method, "GET");
    assert_eq!(mock.object_count(), 2);
}

#[tokio::test]
async fn shared_s3_bucket_rejects_reused_r2_prefix_and_accepts_separate_prefixes() {
    let mock = MockS3::spawn("open-compute").await;
    let first = client_for(&mock).await;
    let instance_a = InstanceId::generate();
    let instance_b = InstanceId::generate();
    preflight_object_storage(&first, instance_a, StartupId::generate())
        .await
        .unwrap();

    let mut config = s3_config(&mock.endpoint);
    config.prefix = "other-system/".to_owned();
    let credentials = resolve_s3_credentials_with(&config, &env()).unwrap();
    let reused_r2 = ObjectBackend::connect_s3(&config, &credentials, 64 * 1024).unwrap();
    assert_eq!(
        preflight_object_storage(&reused_r2, instance_b, StartupId::generate())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ObjectStorageAuthorityMismatch
    );
    assert!(
        crate::verify_object_authority(&first, instance_a)
            .await
            .is_ok()
    );
    let r2_marker =
        crate::ObjectKey::new(format!("{}authority/v1.json", first.r2_prefix())).unwrap();
    first.delete(&r2_marker).await.unwrap();
    assert_eq!(
        preflight_object_storage(&first, instance_a, StartupId::generate())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ObjectStorageAuthorityMismatch
    );

    config.r2_prefix = "other-r2/".to_owned();
    let separated = ObjectBackend::connect_s3(&config, &credentials, 64 * 1024).unwrap();
    preflight_object_storage(&separated, instance_b, StartupId::generate())
        .await
        .unwrap();
    assert!(
        crate::verify_object_authority(&separated, instance_b)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn separate_s3_prefixes_keep_colliding_resource_ids_and_keys_apart() {
    let mock = MockS3::spawn("open-compute").await;
    let first = client_for(&mock).await;
    let mut config = s3_config(&mock.endpoint);
    config.prefix = "other-system/".to_owned();
    config.r2_prefix = "other-r2/".to_owned();
    let credentials = resolve_s3_credentials_with(&config, &env()).unwrap();
    let second = ObjectBackend::connect_s3(&config, &credentials, 64 * 1024).unwrap();
    preflight_object_storage(&first, InstanceId::generate(), StartupId::generate())
        .await
        .unwrap();
    preflight_object_storage(&second, InstanceId::generate(), StartupId::generate())
        .await
        .unwrap();

    let id = open_compute_core::ResourceId::generate();
    for (backend, body) in [(&first, "alpha"), (&second, "beta")] {
        let prefix = crate::R2ObjectStore::new(backend.clone()).physical_prefix(id);
        let key = crate::ObjectKey::new(format!("{prefix}objects/shared-key")).unwrap();
        backend
            .put(
                &key,
                crate::ObjectSource::Bytes(Bytes::copy_from_slice(body.as_bytes())),
                crate::PutOptions {
                    mode: crate::PutMode::CreateOnly,
                    metadata: crate::ObjectMetadata::default(),
                    customer_key: None,
                },
            )
            .await
            .unwrap();
        let read = backend
            .get(&key, crate::GetOptions::default())
            .await
            .unwrap();
        assert_eq!(
            read.body.collect().await.unwrap().into_bytes().as_ref(),
            body.as_bytes()
        );
    }
}
