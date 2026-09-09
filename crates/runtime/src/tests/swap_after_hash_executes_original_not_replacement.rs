use super::*;

#[tokio::test]
async fn swap_after_hash_executes_original_not_replacement() {
    let dir = TempDir::new().unwrap();
    let original = dir.path().join("workerd");
    let marker = dir.path().join("replacement-ran");
    write_exec(&original, &format!("#!/bin/sh\necho '{VERSION}'\n"));
    let lock_path = write_lock(dir.path(), &sha256_file(&original));
    let replacement = dir.path().join("replacement");
    write_exec(
        &replacement,
        &format!(
            "#!/bin/sh\necho swapped > '{}'\necho '{VERSION}'\n",
            marker.display()
        ),
    );
    let orig_keep = dir.path().join("orig-keep");
    set_exec_hook({
        let original = original.clone();
        let orig_keep = orig_keep.clone();
        let replacement = replacement.clone();
        move || {
            let _ = fs::copy(&original, &orig_keep);
            let _ = fs::rename(&replacement, &original);
        }
    });
    let verified = verify_runtime_binary(
        &lock_path,
        &original,
        Duration::from_secs(5),
        &Redactor::new(),
    )
    .await
    .expect("original fd must still execute");
    clear_exec_hook();
    assert_eq!(verified.version_output(), VERSION);
    assert!(
        !marker.exists(),
        "replacement executable must never have run"
    );
}
