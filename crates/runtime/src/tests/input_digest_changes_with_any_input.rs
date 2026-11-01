use super::*;

#[test]
fn input_digest_changes_with_any_input() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let dummy = dir.path().join("workerd");
    write_exec(&dummy, &version_script(None));
    let lock_path = write_lock(dir.path(), &sha256_file(&dummy));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let runtime = rt.block_on(verify_ok(&lock_path, &dummy));
    let lock_bytes = runtime.lock_bytes().to_vec();

    fs::write(dir.path().join("config.capnp"), [0xff]).unwrap();
    assert_eq!(
        digest_for(
            dir.path(),
            &lock_bytes,
            &runtime,
            &platform_meta(),
            Path::new("/tmp/oc-runtime.sock"),
        )
        .unwrap_err()
        .code(),
        ErrorCode::ConfigCompileFailed
    );
    copy_formal_assets(dir.path());
    let (d1, _, _) = digest_for(
        dir.path(),
        &lock_bytes,
        &runtime,
        &platform_meta(),
        Path::new("/tmp/oc-runtime.sock"),
    )
    .unwrap();
    let (d2, _, _) = digest_for(
        dir.path(),
        &lock_bytes,
        &runtime,
        &platform_meta(),
        Path::new("/tmp/oc-runtime.sock"),
    )
    .unwrap();
    assert_eq!(d1, d2);

    fs::write(
        dir.path().join("config.capnp"),
        fs::read_to_string(dir.path().join("config.capnp")).unwrap() + "\n# change\n",
    )
    .unwrap();
    let (d_cfg, _, _) = digest_for(
        dir.path(),
        &lock_bytes,
        &runtime,
        &platform_meta(),
        Path::new("/tmp/oc-runtime.sock"),
    )
    .unwrap();
    assert_ne!(d1, d_cfg);
    copy_formal_assets(dir.path());

    fs::write(
        dir.path().join("dist/gateway/ingress.js"),
        fs::read(dir.path().join("dist/gateway/ingress.js")).unwrap(),
    )
    .unwrap();
    let extra = dir.path().join("dist/extra.js");
    fs::write(
        &extra,
        b"export default {fetch(){return new Response('x')}}",
    )
    .unwrap();
    let (d_w, _, _) = digest_for(
        dir.path(),
        &lock_bytes,
        &runtime,
        &platform_meta(),
        Path::new("/tmp/oc-runtime.sock"),
    )
    .unwrap();
    assert_ne!(d1, d_w);
    fs::remove_file(&extra).unwrap();

    let mut lock_bytes2 = lock_bytes.clone();
    lock_bytes2.extend_from_slice(b" ");
    let (d_lock, rendered, workers) = digest_for(
        dir.path(),
        &lock_bytes,
        &runtime,
        &platform_meta(),
        Path::new("/tmp/oc-runtime.sock"),
    )
    .unwrap();
    let d_lock2 = config_input_digest(&DigestInputs {
        role: crate::ConfigRole::Runtime,
        config_template: &fs::read(dir.path().join("config.capnp")).unwrap(),
        workers: &workers,
        lock_bytes: &lock_bytes2,
        runtime: &runtime,
        platform: &platform_meta(),
        rendered: rendered.as_bytes(),
    });
    assert_ne!(d_lock, d_lock2);
    let prepare_digest = config_input_digest(&DigestInputs {
        role: crate::ConfigRole::PythonPreparation,
        config_template: &fs::read(dir.path().join("config.capnp")).unwrap(),
        workers: &workers,
        lock_bytes: &lock_bytes,
        runtime: &runtime,
        platform: &platform_meta(),
        rendered: rendered.as_bytes(),
    });
    assert_ne!(d_lock, prepare_digest);
    assert_eq!(crate::ConfigRole::Runtime.constant(), "config");
    assert_eq!(
        crate::ConfigRole::PythonPreparation.constant(),
        "prepareConfig"
    );

    let runtime2 = runtime.clone().with_binary_sha256("cd".repeat(32));
    let (d_bin, _, _) = digest_for(
        dir.path(),
        &lock_bytes,
        &runtime2,
        &platform_meta(),
        Path::new("/tmp/oc-runtime.sock"),
    )
    .unwrap();
    assert_ne!(d1, d_bin);

    let (socket_digest, _, _) = digest_for(
        dir.path(),
        &lock_bytes,
        &runtime,
        &platform_meta(),
        Path::new("/tmp/other.sock"),
    )
    .unwrap();
    assert_ne!(d1, socket_digest);

    let meta2 = PlatformReleaseMeta {
        version: "other".into(),
    };
    let (d_rel, _, _) = digest_for(
        dir.path(),
        &lock_bytes,
        &runtime,
        &meta2,
        Path::new("/tmp/oc-runtime.sock"),
    )
    .unwrap();
    assert_ne!(d1, d_rel);
}
