use super::*;
use crate::run::daemon_control::{ControlRequest, DaemonApi, DaemonSocket, RegisteredTokens};
use crate::service_manager::SystemdManager;
use open_compute_core::{InstanceSelector, SecretString};
use std::time::SystemTime;
use tokio::sync::watch;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scoped_cli_lifecycle_uses_one_daemon_socket_and_preserves_data() {
    let temp = tempfile::Builder::new()
        .prefix("scoped-")
        .tempdir_in("/tmp")
        .unwrap();
    let root = temp.path().join("ocd");
    let registry = InstanceRegistry::with_roots(temp.path().join("system"), root.clone());
    let config = temp.path().join("compute.toml");
    let data = temp.path().join("data");
    crate::setup::create_instance(&root, ServiceScope::User, &config, &data, None).unwrap();
    let record = registry
        .register(&config, ServiceScope::User, SystemTime::now())
        .unwrap();
    let id = record.instance_id().unwrap();
    let selector = InstanceSelector::from(id);
    let (api, mut commands) = DaemonApi::channel(
        std::slice::from_ref(&record),
        vec![RegisteredTokens {
            instance_id: id,
            deployer: SecretString::new("deployer"),
            read_only: SecretString::new("reader"),
        }],
        SecretString::new("admin"),
    )
    .unwrap();
    let socket = DaemonSocket::bind(&root).unwrap();
    let (stop, receiver) = watch::channel(false);
    let server = tokio::spawn(socket.serve(api.clone(), receiver));
    let daemon_api = api.clone();
    let daemon_registry = registry.clone();
    let handler = tokio::spawn(async move {
        for _ in 0..5 {
            let command = commands.recv().await.unwrap();
            let result = match command.request {
                ControlRequest::Start { instance_id } => {
                    daemon_api.mark(&instance_id, "running", None)
                }
                ControlRequest::Stop { instance_id } => {
                    daemon_api.mark(&instance_id, "stopped", None)
                }
                ControlRequest::Remove { instance_id } => {
                    daemon_registry.remove_record(&record)?;
                    daemon_api.remove(&instance_id)
                }
                _ => panic!("unexpected lifecycle request"),
            };
            command.reply.send(result.map(|()| None)).unwrap();
        }
        Ok::<(), PlatformError>(())
    });
    let mut output = Vec::new();
    manage_registered_instance(
        &registry,
        ServiceScope::User,
        &selector,
        ScopedInstanceAction::Start,
        &mut output,
    )
    .await
    .unwrap();
    manage_registered_instance(
        &registry,
        ServiceScope::User,
        &selector,
        ScopedInstanceAction::Restart,
        &mut output,
    )
    .await
    .unwrap();
    remove_registered_instance(&registry, ServiceScope::User, &selector, &mut output)
        .await
        .unwrap();
    handler.await.unwrap().unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("INSTANCE_STARTED"));
    assert!(output.contains("INSTANCE_RESTARTED"));
    assert!(output.contains("INSTANCE_REMOVED"));
    assert!(registry.list_scope(ServiceScope::User).unwrap().is_empty());
    assert!(config.exists() && data.join("control.sqlite").exists());
    stop.send(true).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn online_setup_uses_explicit_data_path_without_scanning_instances() {
    let temp = tempfile::Builder::new()
        .prefix("scoped-setup-")
        .tempdir_in("/tmp")
        .unwrap();
    let root = temp.path().join("ocd");
    std::fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let registry = InstanceRegistry::with_roots(temp.path().join("system"), root.clone());
    let config = root.join("instances/alpha/compute.toml");
    let data = root.join("instances/alpha/state");
    let pending_config = root.join("instances/beta/compute.toml");
    let pending_data = root.join("instances/beta/state");
    crate::setup::create_instance(
        &root,
        ServiceScope::User,
        &pending_config,
        &pending_data,
        None,
    )
    .unwrap();
    assert!(registry.list_scope(ServiceScope::User).unwrap().is_empty());
    let (api, mut commands) = DaemonApi::channel(&[], vec![], SecretString::new("admin")).unwrap();
    let socket = DaemonSocket::bind(&root).unwrap();
    let (stop, receiver) = watch::channel(false);
    let server = tokio::spawn(socket.serve(api.clone(), receiver));
    assert_eq!(
        setup_instance(
            &registry,
            ServiceScope::User,
            Some(&root.join("instances/gamma/compute.toml")),
            None,
            None,
            false,
            false,
            false,
            temp.path(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err()
        .code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        setup_instance(
            &registry,
            ServiceScope::User,
            Some(&root.join("instances/gamma/compute.toml")),
            None,
            Some(&root.join("gateway/forbidden")),
            true,
            false,
            false,
            temp.path(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err()
        .code(),
        ErrorCode::PathInvalid
    );
    assert!(!root.join("instances/gamma").exists());
    let daemon_registry = registry.clone();
    let daemon_api = api.clone();
    let daemon_root = root.clone();
    let handler = tokio::spawn(async move {
        for _ in 0..2 {
            let command = commands.recv().await.unwrap();
            let result = match command.request {
                ControlRequest::Create {
                    config_path,
                    data_dir,
                    name,
                    autostart,
                    start,
                } => {
                    assert_eq!(config_path, config);
                    assert_eq!(data_dir, data);
                    assert_eq!(name.unwrap().as_str(), "alpha");
                    assert!(autostart);
                    assert!(!start);
                    crate::setup::create_instance(
                        &daemon_root,
                        ServiceScope::User,
                        &config_path,
                        &data_dir,
                        None,
                    )?;
                    let record = daemon_registry.register(
                        &config_path,
                        ServiceScope::User,
                        SystemTime::now(),
                    )?;
                    daemon_api.insert(
                        &record,
                        RegisteredTokens {
                            instance_id: record.instance_id()?,
                            deployer: SecretString::new("deployer"),
                            read_only: SecretString::new("reader"),
                        },
                    )
                }
                ControlRequest::Add { config_path } => {
                    assert_eq!(config_path, pending_config);
                    let record = daemon_registry.register(
                        &config_path,
                        ServiceScope::User,
                        SystemTime::now(),
                    )?;
                    let id = record.instance_id()?;
                    daemon_api.insert(
                        &record,
                        RegisteredTokens {
                            instance_id: id,
                            deployer: SecretString::new("deployer-beta"),
                            read_only: SecretString::new("reader-beta"),
                        },
                    )?;
                    daemon_api.mark(&id, "running", None)
                }
                _ => panic!("unexpected setup request"),
            };
            command.reply.send(result.map(|()| None)).unwrap();
        }
        Ok::<(), PlatformError>(())
    });
    let mut output = Vec::new();
    setup_instance(
        &registry,
        ServiceScope::User,
        None,
        Some(&"alpha".parse().unwrap()),
        Some(&root.join("instances/alpha/state")),
        true,
        true,
        false,
        temp.path(),
        &mut output,
    )
    .await
    .unwrap();
    let record = registry
        .list_scope(ServiceScope::User)
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(
        record.data_path,
        root.join("instances/alpha/state").to_string_lossy()
    );
    assert!(
        String::from_utf8(output)
            .unwrap()
            .contains("INSTANCE_CREATED")
    );
    let mut added = Vec::new();
    add_registered_instance(
        &registry,
        ServiceScope::User,
        &root.join("instances/beta/compute.toml"),
        temp.path(),
        &mut added,
    )
    .await
    .unwrap();
    handler.await.unwrap().unwrap();
    assert_eq!(registry.list_scope(ServiceScope::User).unwrap().len(), 2);
    assert!(String::from_utf8(added).unwrap().contains("INSTANCE_ADDED"));
    stop.send(true).unwrap();
    server.await.unwrap().unwrap();
}

#[test]
fn stopped_scoped_daemon_requires_both_service_and_socket_to_be_inactive() {
    let temp = tempfile::tempdir().unwrap();
    let registry =
        InstanceRegistry::with_roots(temp.path().join("system"), temp.path().join("user"));
    let manager = SystemdManager {
        unit_root: Some(temp.path().join("units")),
    };
    wait_scoped_daemon_state(&registry, &manager, ServiceScope::User, false).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scoped_cli_fails_closed_on_daemon_rejection_and_failed_runtime() {
    let temp = tempfile::Builder::new()
        .prefix("scoped-errors-")
        .tempdir_in("/tmp")
        .unwrap();
    let root = temp.path().join("ocd");
    let registry = InstanceRegistry::with_roots(temp.path().join("system"), root.clone());
    let config = temp.path().join("compute.toml");
    let data = temp.path().join("data");
    crate::setup::create_instance(&root, ServiceScope::User, &config, &data, None).unwrap();
    let record = registry
        .register(&config, ServiceScope::User, SystemTime::now())
        .unwrap();
    let id = record.instance_id().unwrap();
    let selector = InstanceSelector::from(id);
    let (api, mut commands) = DaemonApi::channel(
        &[record],
        vec![RegisteredTokens {
            instance_id: id,
            deployer: SecretString::new("deployer"),
            read_only: SecretString::new("reader"),
        }],
        SecretString::new("admin"),
    )
    .unwrap();
    let socket = DaemonSocket::bind(&root).unwrap();
    let (stop, receiver) = watch::channel(false);
    let server = tokio::spawn(socket.serve(api.clone(), receiver));
    let daemon_api = api.clone();
    let handler = tokio::spawn(async move {
        for attempt in 0..2 {
            let command = commands.recv().await.unwrap();
            assert!(matches!(command.request, ControlRequest::Start { .. }));
            let result = if attempt == 0 {
                Err(PlatformError::new(ErrorCode::SecretRefInvalid, "rejected"))
            } else {
                daemon_api.mark(&id, "failed", Some(ErrorCode::RuntimeUnavailable))?;
                Ok(None)
            };
            command.reply.send(result).unwrap();
        }
        Ok::<(), PlatformError>(())
    });
    for code in [ErrorCode::SecretRefInvalid, ErrorCode::RuntimeUnavailable] {
        assert_eq!(
            manage_registered_instance(
                &registry,
                ServiceScope::User,
                &selector,
                ScopedInstanceAction::Start,
                &mut Vec::new(),
            )
            .await
            .unwrap_err()
            .code(),
            code
        );
    }
    handler.await.unwrap().unwrap();
    api.remove(&id).unwrap();
    assert_eq!(
        remove_registered_instance(&registry, ServiceScope::User, &selector, &mut Vec::new())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::InstanceNotFound
    );
    assert!(config.exists() && data.join("control.sqlite").exists());
    stop.send(true).unwrap();
    server.await.unwrap().unwrap();
}
