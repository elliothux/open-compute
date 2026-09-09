use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_same_digest_publish_reuses_one_winner() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let counter = dir.path().join("count");
    let args = dir.path().join("args");
    let bin = dir.path().join("workerd");
    write_exec(
        &bin,
        &format!(
            "#!/bin/sh
printf x >> '{counter}'
printf '%s\\n' \"$0\" \"$@\" > '{args}'
if [ \"$1\" = \"--version\" ]; then
  echo '{VERSION}'
  exit 0
fi
sleep 0.15
printf 'COMPILED-%s' \"$$\"
",
            counter = counter.display(),
            args = args.display(),
            VERSION = VERSION,
        ),
    );
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let redactor = redactor_with_token();
    let platform = platform_meta();
    let dest_name = {
        let (digest, _, _) = digest_for(
            dir.path(),
            runtime.lock_bytes(),
            &runtime,
            &platform,
            &token,
            &TEST_BINDING_TOKEN,
            &TEST_OBSERVABILITY_TOKEN,
        )
        .unwrap();
        format!("config.{digest}.bin")
    };
    let dest = data.join(&dest_name);
    let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    set_exec_hook({
        let started = started.clone();
        move || {
            started.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let wait_from = std::time::Instant::now();
            while started.load(std::sync::atomic::Ordering::SeqCst) < 2
                && wait_from.elapsed() < Duration::from_secs(2)
            {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    });
    let published = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    set_publish_hook({
        let dest = dest.clone();
        let published = published.clone();
        move |path| {
            if path == dest {
                published.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
    });
    let a = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    ));
    let b = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    ));
    let (ra, rb) = tokio::join!(a, b);
    clear_publish_hook();
    clear_exec_hook();
    let ca = ra.expect("first compile");
    let cb = rb.expect("second compile");
    assert_eq!(ca.digest(), cb.digest());
    assert_eq!(ca.path(), cb.path());
    let mut fa = ca.open().unwrap();
    let mut fb = cb.open().unwrap();
    let mut ba = Vec::new();
    let mut bb = Vec::new();
    std::io::Read::read_to_end(&mut fa, &mut ba).unwrap();
    std::io::Read::read_to_end(&mut fb, &mut bb).unwrap();
    assert_eq!(ba, bb);
    assert!(
        ba.starts_with(b"COMPILED-"),
        "winner must be one compile payload: {:?}",
        String::from_utf8_lossy(&ba)
    );
    let sidecar = fs::read_to_string(ca.path().with_extension("bin.digest")).unwrap();
    assert!(sidecar.starts_with(ca.digest()));
    assert!(sidecar.contains(&sha256_bytes(&ba)));
    assert_eq!(
        started.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "both compilers must compile concurrently"
    );
    assert_eq!(
        published.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "only the winning no-replace rename should publish the dest config"
    );
}
