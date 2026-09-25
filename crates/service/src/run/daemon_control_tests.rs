use super::*;
use crate::instance_registry::{RegisteredObjectAuthority, ServiceScope};
use open_compute_core::{CacheConfig, StartupId, SystemClock, config::DataConfig};

#[test]
fn daemon_api_scopes_tokens_and_refreshes_only_stopped_instances() {
    let id = InstanceId::generate();
    let mut record = InstanceRecord {
        instance_id: id.to_string(),
        name: Some("first".to_owned()),
        canonical_config_path: "/first.toml".to_owned(),
        config_sha256: "00".repeat(32),
        data_path: "/first-data".to_owned(),
        object_authority: RegisteredObjectAuthority::Local,
        public_base_domain: None,
        service_scope: ServiceScope::User,
        created_at: 1,
        autostart: false,
    };
    let tokens = |instance_id, deployer: &str| RegisteredTokens {
        instance_id,
        deployer: SecretString::new(deployer),
        read_only: SecretString::new("reader"),
    };
    assert!(DaemonApi::channel(&[record.clone()], vec![], SecretString::new("admin")).is_err());
    let (api, _commands) = DaemonApi::channel(
        &[record.clone()],
        vec![tokens(id, "deployer")],
        SecretString::new("admin"),
    )
    .unwrap();
    assert!(api.authorized(Some("Bearer admin")));
    assert!(!api.authorized(Some("Bearer deployer")));
    assert!(matches!(
        api.visible_for_bearer(Some("Bearer admin")).unwrap(),
        Some((_, V4Role::Admin))
    ));
    assert!(matches!(
        api.visible_for_bearer(Some("Bearer deployer")).unwrap(),
        Some((_, V4Role::Deployer))
    ));
    assert!(matches!(
        api.visible_for_bearer(Some("Bearer reader")).unwrap(),
        Some((_, V4Role::ReadOnly))
    ));
    assert!(
        api.visible_for_bearer(Some("Bearer wrong"))
            .unwrap()
            .is_none()
    );
    assert!(
        api.matches_runtime(
            &id,
            &SecretString::new("admin"),
            &SecretString::new("deployer"),
            &SecretString::new("reader"),
            None,
        )
        .unwrap()
    );
    assert!(
        !api.matches_runtime(
            &id,
            &SecretString::new("wrong"),
            &SecretString::new("deployer"),
            &SecretString::new("reader"),
            None,
        )
        .unwrap()
    );
    record.name = Some("renamed".to_owned());
    api.mark(&id, "running", None).unwrap();
    assert!(api.refresh_stopped(&record, tokens(id, "new")).is_err());
    api.mark(&id, "failed", Some(ErrorCode::RuntimeUnavailable))
        .unwrap();
    api.refresh_stopped(&record, tokens(id, "new")).unwrap();
    let view = api.list().unwrap().remove(0);
    assert_eq!(view.name.as_deref(), Some("renamed"));
    assert_eq!(view.error.as_deref(), Some("RUNTIME_UNAVAILABLE"));
    assert!(api.insert(&record, tokens(id, "new")).is_err());
    api.remove(&id).unwrap();
    assert_eq!(
        api.remove(&id).unwrap_err().code(),
        ErrorCode::InstanceNotFound
    );
    assert!(api.list().unwrap().is_empty());
}

#[tokio::test]
async fn control_socket_rejects_unowned_or_invalid_requests() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ocd");
    fs::create_dir(&root).unwrap();
    let (api, _commands) = DaemonApi::channel(&[], vec![], SecretString::new("admin")).unwrap();
    let socket = DaemonSocket::bind(&root).unwrap();
    assert_eq!(
        DaemonSocket::bind(&root).err().unwrap().code(),
        ErrorCode::InstanceRegistryInvalid
    );
    let (stop, receiver) = watch::channel(false);
    let server = tokio::spawn(socket.serve(api, receiver));
    let root_for_client = root.clone();
    tokio::task::spawn_blocking(move || {
        let response = exchange(&root_for_client, &ControlRequest::List).unwrap();
        assert!(response.ok);
        assert!(response.instances.unwrap().is_empty());
        for request in [
            ControlRequest::CaddyStatus,
            ControlRequest::CaddyReload,
            ControlRequest::CaddyValidate,
        ] {
            let response = exchange(&root_for_client, &request).unwrap();
            assert!(!response.ok);
            assert_eq!(response.error.as_deref(), Some("CONFIG_INVALID"));
        }
        let mut stream = UnixStream::connect(root_for_client.join("run/control.sock")).unwrap();
        stream.write_all(b"{invalid}\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.contains("INSTANCE_REGISTRY_INVALID"));
        let path = root_for_client.join("run/control.sock");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            exchange(&root_for_client, &ControlRequest::List)
                .err()
                .unwrap()
                .code(),
            ErrorCode::InstanceRegistryInvalid
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    })
    .await
    .unwrap();
    stop.send(true).unwrap();
    server.await.unwrap().unwrap();
    assert!(!root.join("run/control.sock").exists());
}

#[tokio::test]
async fn cache_clean_uses_registered_live_cache_and_reports_dry_run() {
    let temp = tempfile::tempdir().unwrap();
    let data = DataConfig {
        path: temp.path().join("data"),
        master_key_file: temp.path().join("data/keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 2,
        free_space_hard_bytes: 1,
    };
    let storage = Arc::new(PlatformStorage::bootstrap(&data, &SystemClock).unwrap());
    let id = storage.identity().instance_id;
    let record = InstanceRecord {
        instance_id: id.to_string(),
        name: Some("dev".to_owned()),
        canonical_config_path: temp.path().join("compute.toml").display().to_string(),
        config_sha256: "00".repeat(32),
        data_path: temp.path().join("data").display().to_string(),
        object_authority: RegisteredObjectAuthority::Local,
        public_base_domain: None,
        service_scope: ServiceScope::User,
        created_at: 1,
        autostart: true,
    };
    let tokens = RegisteredTokens {
        instance_id: id,
        deployer: SecretString::new("deployer"),
        read_only: SecretString::new("readonly"),
    };
    let (api, mut commands) =
        DaemonApi::channel(&[record], vec![tokens], SecretString::new("admin")).unwrap();
    assert!(api.clean_cache(&id, true).await.is_err());

    let root = storage.data_dir().artifact_cache_dir();
    let digest = "ab".repeat(32);
    let shard = root.join("sha256/ab");
    fs::create_dir_all(&shard).unwrap();
    let entry = shard.join(&digest[2..]);
    fs::write(&entry, b"copy").unwrap();
    let cache =
        Arc::new(ArtifactCache::open(root, CacheConfig::default(), StartupId::generate()).unwrap());
    assert!(
        api.register_cache(InstanceId::generate(), &cache, &storage)
            .is_err()
    );
    api.register_cache(id, &cache, &storage).unwrap();
    assert_eq!(api.clean_cache(&id, true).await.unwrap().bytes, 4);
    assert!(entry.exists());

    let ocd_root = temp.path().join("ocd");
    fs::create_dir(&ocd_root).unwrap();
    let socket = DaemonSocket::bind(&ocd_root).unwrap();
    let (stop, stopped) = watch::channel(false);
    let server = tokio::spawn(socket.serve(api.clone(), stopped));
    let owner = api.clone();
    let handler = tokio::spawn(async move {
        let command = commands.recv().await.unwrap();
        let result = match command.request {
            ControlRequest::CleanCache {
                instance_id,
                dry_run,
            } => owner.clean_cache(&instance_id, dry_run).await.map(Some),
            _ => unreachable!(),
        };
        command.reply.send(result).unwrap();
    });
    let response = tokio::task::spawn_blocking(move || {
        exchange(
            &ocd_root,
            &ControlRequest::CleanCache {
                instance_id: id,
                dry_run: false,
            },
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert!(response.ok);
    assert_eq!(response.cache_report.unwrap().bytes, 4);
    assert!(!entry.exists());
    handler.await.unwrap();
    stop.send(true).unwrap();
    server.await.unwrap().unwrap();
    drop(storage);
    assert!(api.clean_cache(&id, false).await.is_err());
    drop(cache);
    assert!(api.clean_cache(&id, false).await.is_err());
}

#[tokio::test]
async fn global_cache_clean_socket_preserves_online_update_cache() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ocd");
    fs::create_dir(&root).unwrap();
    let packages = root.join("cache/packages");
    let old = packages.join("ab".repeat(32));
    fs::create_dir_all(old.join("runtime")).unwrap();
    fs::write(old.join("runtime/workerd.lock.json"), b"{}").unwrap();
    fs::write(old.join("workerd"), b"tool").unwrap();
    let update = root.join("cache/update-check.json");
    fs::write(&update, b"active").unwrap();
    let (api, mut commands) = DaemonApi::channel(&[], vec![], SecretString::new("admin")).unwrap();
    let socket = DaemonSocket::bind(&root).unwrap();
    let (stop, stopped) = watch::channel(false);
    let server = tokio::spawn(socket.serve(api, stopped));
    let owner_root = root.clone();
    let handler = tokio::spawn(async move {
        let command = commands.recv().await.unwrap();
        let result = match command.request {
            ControlRequest::CleanGlobalCache { dry_run } => {
                crate::run::clean_global_cache(&owner_root, dry_run, true).map(Some)
            }
            _ => unreachable!(),
        };
        command.reply.send(result).unwrap();
    });
    let response = tokio::task::spawn_blocking(move || {
        exchange(&root, &ControlRequest::CleanGlobalCache { dry_run: false })
    })
    .await
    .unwrap()
    .unwrap();
    assert!(response.ok);
    let report = response.cache_report.unwrap();
    assert_eq!(report.bytes, 6);
    assert_eq!(report.entries, 1);
    assert!(report.skipped >= 1);
    assert!(!old.exists());
    assert!(update.exists());
    handler.await.unwrap();
    stop.send(true).unwrap();
    server.await.unwrap().unwrap();
}
