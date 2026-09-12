use super::*;

#[tokio::test]
async fn run_real_workerd_on_merged_listener_serves_status_and_shuts_down() {
    let (_dir, path, mock) = initialized_doctor_fixture().await;
    let mut loaded = load_fixture_platform_config(&path);
    loaded.config.runtime.startup_timeout_ms = 60_000;
    loaded.config.runtime.shutdown_grace_ms = 1_000;
    loaded.config.runtime.kill_timeout_ms = 2_000;

    let reserved = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reserved.local_addr().unwrap();
    drop(reserved);
    loaded.config.server.public_bind = address.to_string();
    loaded.config.server.admin_bind = None;

    let registry = InstanceRegistry::with_roots(
        _dir.path().join("registry/system"),
        _dir.path().join("registry/user"),
    );
    let mut task = tokio::spawn(run_platform_with(
        loaded,
        RunOptions {
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
                    stream.read_to_end(&mut response).await.unwrap();
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
    assert_eq!(mock.object_count(), 1);
}
