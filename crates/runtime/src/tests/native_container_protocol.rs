use super::*;

#[tokio::test]
async fn native_container_protocol_requires_the_verified_executable_contract() {
    for (command, accepted) in [
        ("printf 'open-compute-container-protocol-v4\\n'", true),
        ("printf 'open-compute-container-protocol-v3\\n'", false),
        ("printf 'open-compute-container-protocol-v2\\n'", false),
        ("printf 'open-compute-container-protocol-v1\\n'", false),
        ("printf 'open-compute-container-protocol-v5\\n'", false),
        (
            "printf 'open-compute-container-protocol-v4\\n'; exit 1",
            false,
        ),
        ("printf 'open-compute-container-protocol-v4\\nextra'", false),
        ("printf '%0200d' 0", false),
    ] {
        let dir = TempDir::new().unwrap();
        let binary = dir.path().join("workerd");
        write_exec(
            &binary,
            &format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo '{VERSION}'; else {command}; fi\n"
            ),
        );
        let lock = write_lock(dir.path(), &sha256_file(&binary));
        let runtime = verify_ok(&lock, &binary).await;
        let result = runtime
            .check_container_protocol(Duration::from_secs(5), &Redactor::new())
            .await;
        assert_eq!(result.is_ok(), accepted);
        if accepted {
            let assets = dir.path().join("assets");
            copy_formal_assets(&assets);

            let platform = platform_meta();
            let base = crate::digest::digest_for_with_policy(
                &assets,
                runtime.lock_bytes(),
                &runtime,
                &platform,
                Path::new("/tmp/oc-runtime.sock"),
                &open_compute_core::DurableObjectsConfig::default(),
                None,
                crate::ConfigRole::Runtime,
            )
            .unwrap();
            let provider = crate::ContainerProviderConfig::new(
                "/tmp/broker.sock".into(),
                format!("sha256:{}", "a".repeat(64)),
            )
            .unwrap();
            let configured = crate::digest::digest_for_with_policy(
                &assets,
                runtime.lock_bytes(),
                &runtime,
                &platform,
                Path::new("/tmp/oc-runtime.sock"),
                &open_compute_core::DurableObjectsConfig::default(),
                Some(&provider),
                crate::ConfigRole::Runtime,
            )
            .unwrap();
            assert_ne!(base.0, configured.0);
            assert!(
                configured
                    .1
                    .contains("socketPath = \"unix:/tmp/broker.sock\"")
            );
            assert!(!configured.1.contains("staticContainerOptions"));
            let _compiler = crate::StaticConfigCompiler::new(
                runtime.clone(),
                lock.clone(),
                assets,
                dir.path().join("runtime"),
                dir.path().join("runtime").join("ingress.sock"),
                platform,
                Duration::from_secs(5),
                Redactor::new(),
            )
            .with_container_provider(provider);
        }
        if let Err(error) = result {
            assert_eq!(error.code(), ErrorCode::RuntimeInvalid);
        }
    }
}
