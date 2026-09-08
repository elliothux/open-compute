use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compile_stdout_streams_into_partial_before_exit() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let counter = dir.path().join("count");
    let args = dir.path().join("args");
    let go = dir.path().join("stream-go");
    let bin = dir.path().join("workerd");
    write_exec(
        &bin,
        &compile_script(
            &counter,
            &args,
            "CHUNK2",
            &format!(
                "printf 'CHUNK1-'\nwhile [ ! -f '{}' ]; do sleep 0.05; done\n",
                go.display()
            ),
        ),
    );
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let platform = platform_meta();
    let compile = tokio::spawn({
        let runtime = runtime.clone();
        let lock_path = lock_path.clone();
        let assets = dir.path().to_path_buf();
        let data = data.clone();
        let platform = platform.clone();
        let token = SecretString::new(TOKEN);
        async move {
            compile_static_config(compile_req(
                &runtime,
                &lock_path,
                &assets,
                &data,
                &platform,
                &token,
                &redactor_with_token(),
                Duration::from_secs(8),
            ))
            .await
        }
    });
    let started = std::time::Instant::now();
    let partial = loop {
        if let Some(path) = find_partial_config(&data)
            && fs::read(&path).is_ok_and(|b| b.starts_with(b"CHUNK1-"))
        {
            break path;
        }
        if compile.is_finished() {
            panic!("compile finished before streaming CHUNK1 into the partial");
        }
        if started.elapsed() > Duration::from_secs(4) {
            panic!("partial did not grow before child exit");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(
        !compile.is_finished(),
        "child must still be running while the partial already contains streamed bytes"
    );
    fs::write(&go, b"go").unwrap();
    let compiled = compile.await.expect("join").expect("compile");
    assert_eq!(fs::read(compiled.path()).unwrap(), b"CHUNK1-CHUNK2");
    compiled.open().expect("revalidate streamed compile output");
    let _ = partial;
}
