use super::*;

#[tokio::test]
async fn run_real_workerd_on_merged_listener_serves_status_and_shuts_down() {
    let (_dir, path, mock) = initialized_doctor_fixture().await;

    let reserved = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reserved.local_addr().unwrap();
    drop(reserved);

    let https = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let https_addr = https.local_addr().unwrap();
    drop(https);
    let challenge_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let challenge_addr = challenge_tcp.local_addr().unwrap();
    let challenge_udp = std::net::UdpSocket::bind(challenge_addr).unwrap();
    drop((challenge_tcp, challenge_udp));
    let source = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        format!("{source}\n[public_gateway]\nbase_domain = \"compute.example.com\"\n"),
    )
    .unwrap();
    let mut loaded = load_fixture_platform_config(&path);
    loaded.config.runtime.startup_timeout_ms = 60_000;
    loaded.config.runtime.shutdown_grace_ms = 1_000;
    loaded.config.runtime.kill_timeout_ms = 2_000;
    let instance_data = loaded.config.data.path.clone();

    let registry_root = TempDir::new_in("/tmp").unwrap();
    let registry = InstanceRegistry::with_roots(
        registry_root.path().join("system"),
        registry_root.path().join("user"),
    );
    registry
        .register(
            &loaded.path,
            crate::instance_registry::ServiceScope::User,
            SystemTime::now(),
        )
        .unwrap();
    let manifest = registry
        .root_for(crate::instance_registry::ServiceScope::User)
        .join("ocd.toml");
    let source = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        format!(
            "{source}\n[gateway]\ningress_ipv4 = [\"203.0.113.10\"]\nhttps_listen = \"{https_addr}\"\nchallenge_dns_listen = \"{challenge_addr}\"\n"
        ),
    )
    .unwrap();
    let daemon_server = open_compute_core::DaemonServerConfig {
        public_bind: address.to_string(),
        admin_bind: None,
        admin_auth: SecretReference {
            env: None,
            file: Some(_dir.path().join("admin-auth")),
        },
    };
    let mut task = tokio::spawn(run_platform_with(
        loaded,
        RunOptions {
            daemon_server,
            instance_registry: Some(registry),
            ..RunOptions::default()
        },
    ));
    let response = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            match tokio::net::TcpStream::connect(address).await {
                Ok(mut stream) => {
                    stream
                        .write_all(
                            b"GET /client/v4/open-compute/system/status HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-admin-secret\r\nConnection: close\r\n\r\n",
                        )
                        .await
                        .unwrap();
                    let mut response = Vec::new();
                    if stream.read_to_end(&mut response).await.is_err() {
                        if task.is_finished() {
                            panic!("platform startup ended early: {:?}", (&mut task).await);
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        continue;
                    }
                    if response
                        .windows(b"\"name\":\"runtime\",\"state\":\"healthy\"".len())
                        .any(|window| {
                            window == b"\"name\":\"runtime\",\"state\":\"healthy\""
                        })
                    {
                        break response;
                    }
                    if task.is_finished() {
                        panic!("platform startup ended early: {:?}", (&mut task).await);
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(_) => {
                    if task.is_finished() {
                        panic!("platform startup ended early: {:?}", (&mut task).await);
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
    })
    .await
    .expect("merged listener readiness");

    let scope_root = registry_root.path().join("user");
    assert!(scope_root.join("gateway/Caddyfile").is_file());
    assert!(scope_root.join("run/gateway/upstream.sock").exists());
    assert!(!instance_data.join("gateway").exists());

    rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::TERM)
        .unwrap();
    tokio::time::timeout(Duration::from_secs(60), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(
        response.contains("\"name\":\"runtime\",\"state\":\"healthy\""),
        "{response}"
    );
    assert_eq!(mock.object_count(), 2);
}
