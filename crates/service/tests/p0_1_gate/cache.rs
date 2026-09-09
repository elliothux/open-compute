use super::*;

pub(super) async fn cache_survives_s3_outage(s3: &MockS3, data: &Path) {
    let s3_cfg = S3Config {
        endpoint: s3.endpoint.clone(),
        access_key_id_env: Some("OC_S3_ID_1".into()),
        secret_access_key_env: Some("OC_S3_SECRET_1".into()),
        prefix: "artifacts/".into(),
        max_retries: 1,
        connect_timeout_ms: 1000,
        request_timeout_ms: 2000,
        ..S3Config::default()
    };
    let map = MapEnv::new()
        .with("OC_S3_ID_1", "gate-access")
        .with("OC_S3_SECRET_1", "gate-secret-value");
    let creds = resolve_s3_credentials_with(&s3_cfg, &map).expect("resolve Gate S3 credentials");
    let client = ObjectBackend::connect_s3(&s3_cfg, &creds, 65_536).unwrap();
    let store = ArtifactStore::new(client);
    let body = b"immutable-cache-body".to_vec();
    let digest = hex::encode(Sha256::digest(&body));
    let stream = stream::iter([Ok::<_, std::io::Error>(Bytes::from(body.clone()))]);
    let artifact = store
        .put_verified(stream, &digest, body.len() as u64)
        .await
        .expect("put");
    let cache_root = data.join("cache/artifacts");
    let cache = ArtifactCache::open(cache_root, CacheConfig::default(), StartupId::generate())
        .expect("cache");
    let mut pinned = cache.acquire(&store, &artifact).await.expect("cold");
    assert_eq!(pinned.read_all().unwrap(), body);
    s3.set_fault(open_compute_artifacts::Fault::Timeout);
    let mut hit = cache.acquire_cached(&artifact).await.expect("cached hit");
    assert_eq!(hit.read_all().unwrap(), body);
    s3.set_fault(open_compute_artifacts::Fault::None);
    let _ = ArtifactRef::new(1, &digest, body.len() as u64);
}
