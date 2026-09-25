use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[tokio::test]
async fn daemon_commands_enforce_registration_and_instance_lifecycle() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ocd");
    let registry = InstanceRegistry::with_roots(temp.path().join("system"), root.clone());
    let config = temp.path().join("first.toml");
    let data = temp.path().join("first-data");
    crate::setup::create_instance(&root, ServiceScope::User, &config, &data, None).unwrap();
    let admin_path = temp.path().join("admin.token");
    fs::write(&admin_path, "admin").unwrap();
    fs::set_permissions(&admin_path, fs::Permissions::from_mode(0o600)).unwrap();
    let record = registry
        .register_first_with_server(
            &config,
            ServiceScope::User,
            DaemonServerConfig {
                admin_auth: open_compute_core::SecretReference {
                    env: None,
                    file: Some(admin_path),
                },
                ..Default::default()
            },
            SystemTime::now(),
        )
        .unwrap();
    let id = record.instance_id().unwrap();
    let tokens = daemon_control::RegisteredTokens {
        instance_id: id,
        deployer: open_compute_core::SecretString::new("first-deployer"),
        read_only: open_compute_core::SecretString::new("first-reader"),
    };
    let (api, _commands) = daemon_control::DaemonApi::channel(
        std::slice::from_ref(&record),
        vec![tokens],
        open_compute_core::SecretString::new("admin"),
    )
    .unwrap();
    let mut plan = DaemonPlan {
        root: root.clone(),
        scope: ServiceScope::User,
        manifest_digest: crate::instance_registry::manifest_digest(&root).unwrap(),
        records: vec![record],
        registry,
        credentials: Vec::new(),
    };
    let opts = RunInner {
        daemon_server: plan.registry.server_config(ServiceScope::User).unwrap(),
        fail_after: Some(FailAfter::Config),
        ..Default::default()
    };
    let mut tasks = RuntimeTasks::new();
    let mut active = HashMap::new();
    let mut shutdowns = Vec::new();
    let mut dispatch = |request, plan: Option<&mut DaemonPlan>, active: &mut HashMap<_, _>| {
        let (reply, _) = tokio::sync::oneshot::channel();
        handle_daemon_command(
            &daemon_control::DaemonCommand { request, reply },
            plan,
            None,
            &opts,
            &mut tasks,
            active,
            &mut shutdowns,
            Some(&api),
        )
    };
    assert_eq!(
        dispatch(daemon_control::ControlRequest::List, None, &mut active)
            .unwrap_err()
            .code(),
        ErrorCode::RuntimeUnavailable
    );
    for request in [
        daemon_control::ControlRequest::List,
        daemon_control::ControlRequest::CaddyStatus,
        daemon_control::ControlRequest::CaddyReload,
        daemon_control::ControlRequest::CaddyValidate,
        daemon_control::ControlRequest::CleanGlobalCache { dry_run: true },
        daemon_control::ControlRequest::CleanCache {
            instance_id: id,
            dry_run: true,
        },
    ] {
        assert_eq!(
            dispatch(request, Some(&mut plan), &mut active)
                .unwrap_err()
                .code(),
            ErrorCode::ConfigInvalid
        );
    }
    let unknown = InstanceId::generate();
    assert_eq!(
        dispatch(
            daemon_control::ControlRequest::Stop {
                instance_id: unknown,
            },
            Some(&mut plan),
            &mut active,
        )
        .unwrap_err()
        .code(),
        ErrorCode::InstanceNotFound
    );
    assert_eq!(
        dispatch(
            daemon_control::ControlRequest::Stop { instance_id: id },
            Some(&mut plan),
            &mut active,
        )
        .unwrap_err()
        .code(),
        ErrorCode::InstanceRegistryInvalid
    );
    let (shutdown, receiver) = watch::channel(false);
    active.insert(id, shutdown);
    dispatch(
        daemon_control::ControlRequest::Stop { instance_id: id },
        Some(&mut plan),
        &mut active,
    )
    .unwrap();
    assert!(*receiver.borrow());
    for request in [
        daemon_control::ControlRequest::Start { instance_id: id },
        daemon_control::ControlRequest::Remove { instance_id: id },
    ] {
        assert_eq!(
            dispatch(request, Some(&mut plan), &mut active)
                .unwrap_err()
                .code(),
            ErrorCode::InstanceRegistryInvalid
        );
    }
    active.remove(&id);
    assert_eq!(
        dispatch(
            daemon_control::ControlRequest::Remove {
                instance_id: unknown,
            },
            Some(&mut plan),
            &mut active,
        )
        .unwrap_err()
        .code(),
        ErrorCode::InstanceNotFound
    );
    dispatch(
        daemon_control::ControlRequest::Remove { instance_id: id },
        Some(&mut plan),
        &mut active,
    )
    .unwrap();
    assert!(plan.records.is_empty());
    assert!(api.list().unwrap().is_empty());

    let next_config = temp.path().join("next.toml");
    let next_data = temp.path().join("next-data");
    dispatch(
        daemon_control::ControlRequest::Create {
            config_path: next_config.clone(),
            data_dir: next_data.clone(),
            name: None,
            autostart: false,
            start: false,
        },
        Some(&mut plan),
        &mut active,
    )
    .unwrap();
    assert!(next_config.is_file());
    assert!(next_data.join("control.sqlite").is_file());
    assert_eq!(plan.records.len(), 1);
    assert!(!plan.records[0].autostart);
    assert_eq!(api.list().unwrap().len(), 1);
    let next_id = plan.records[0].instance_id().unwrap();
    assert_eq!(
        dispatch(
            daemon_control::ControlRequest::Start {
                instance_id: unknown,
            },
            Some(&mut plan),
            &mut active,
        )
        .unwrap_err()
        .code(),
        ErrorCode::InstanceNotFound
    );
    refresh_stopped_instance(&mut plan, &next_id, None, &opts, &api).unwrap();
    dispatch(
        daemon_control::ControlRequest::Start {
            instance_id: next_id,
        },
        Some(&mut plan),
        &mut active,
    )
    .unwrap();
    assert_eq!(api.list().unwrap()[0].state, "starting");
    active.remove(&next_id);
    let (shutdown, receiver) = watch::channel(false);
    drop(receiver);
    active.insert(next_id, shutdown);
    assert_eq!(
        dispatch(
            daemon_control::ControlRequest::Stop {
                instance_id: next_id,
            },
            Some(&mut plan),
            &mut active,
        )
        .unwrap_err()
        .code(),
        ErrorCode::RuntimeUnavailable
    );
    let extra_config = temp.path().join("extra.toml");
    let extra_data = temp.path().join("extra-data");
    crate::setup::create_instance(&root, ServiceScope::User, &extra_config, &extra_data, None)
        .unwrap();
    dispatch(
        daemon_control::ControlRequest::Add {
            config_path: extra_config.clone(),
        },
        Some(&mut plan),
        &mut active,
    )
    .unwrap();
    assert_eq!(plan.records.len(), 2);
    assert_eq!(api.list().unwrap().len(), 2);
    let original = fs::read(&extra_config).unwrap();
    fs::write(
        &extra_config,
        [original.as_slice(), b"\n# external edit\n"].concat(),
    )
    .unwrap();
    assert_eq!(
        refresh_stopped_instance(&mut plan, &next_id, None, &opts, &api)
            .unwrap_err()
            .code(),
        ErrorCode::InstanceRegistryInvalid
    );
    fs::write(&extra_config, original).unwrap();
    assert!(tasks.join_next().await.unwrap().unwrap().1.is_err());
    assert!(tasks.join_next().await.unwrap().unwrap().1.is_err());
}

#[tokio::test]
async fn stopped_instance_cache_clean_is_locked_scoped_and_dry_run_safe() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ocd");
    let registry = InstanceRegistry::with_roots(temp.path().join("system"), root.clone());
    let config = temp.path().join("compute.toml");
    let data = temp.path().join("data");
    crate::setup::create_instance(&root, ServiceScope::User, &config, &data, None).unwrap();
    let record = registry
        .register(&config, ServiceScope::User, SystemTime::now())
        .unwrap();
    let id = record.instance_id().unwrap();
    let plan = DaemonPlan {
        root: root.clone(),
        scope: ServiceScope::User,
        manifest_digest: crate::instance_registry::manifest_digest(&root).unwrap(),
        records: vec![record],
        registry,
        credentials: Vec::new(),
    };
    let cache = data.join("cache/artifacts/sha256/ab");
    fs::create_dir_all(&cache).unwrap();
    let entry = cache.join("ab".repeat(31));
    fs::write(&entry, b"copy").unwrap();

    let competing = open_compute_storage::InspectLock::try_acquire(&data.join("platform.lock"))
        .unwrap()
        .unwrap();
    assert_eq!(
        clean_stopped_cache(&plan, &id, true)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::DataDirInUse
    );
    drop(competing);

    let dry = clean_stopped_cache(&plan, &id, true).await.unwrap();
    assert_eq!(dry.bytes, 4);
    assert!(entry.exists());
    let clean = clean_stopped_cache(&plan, &id, false).await.unwrap();
    assert_eq!(clean.bytes, 4);
    assert!(!entry.exists());
    assert_eq!(
        clean_stopped_cache(&plan, &id, false).await.unwrap().bytes,
        0
    );
    assert_eq!(
        clean_stopped_cache(&plan, &InstanceId::generate(), true)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::InstanceNotFound
    );

    let providers = data.join("runtime/extensions");
    fs::create_dir_all(providers.join("empty-provider")).unwrap();
    assert_eq!(
        clean_stopped_cache(&plan, &id, true).await.unwrap().bytes,
        0
    );
    let outside = temp.path().join("outside-provider");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, providers.join("linked-provider")).unwrap();
    assert_eq!(
        clean_stopped_cache(&plan, &id, true)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
}
