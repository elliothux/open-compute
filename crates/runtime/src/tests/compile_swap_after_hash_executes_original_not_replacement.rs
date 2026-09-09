use super::*;

#[tokio::test]
async fn compile_swap_after_hash_executes_original_not_replacement() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let marker = dir.path().join("replacement-ran");
    let counter = dir.path().join("count");
    let args = dir.path().join("args");
    let bin = dir.path().join("workerd");
    write_exec(&bin, &compile_script(&counter, &args, "COMPILED-BYTES", ""));
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
    let replacement = dir.path().join("replacement");
    write_exec(
        &replacement,
        &format!(
            "#!/bin/sh\necho swapped > '{}'\necho '{VERSION}'\n",
            marker.display()
        ),
    );
    set_exec_hook({
        let bin = bin.clone();
        let replacement = replacement.clone();
        move || {
            let _ = fs::rename(&replacement, &bin);
        }
    });
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let redactor = redactor_with_token();
    let platform = platform_meta();
    let compiled = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    ))
    .await;
    clear_exec_hook();
    compiled.expect("original verified fd must compile");
    assert!(
        !marker.exists(),
        "replacement executable must never have run"
    );
}
