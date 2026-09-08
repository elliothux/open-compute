use super::*;

#[test]
fn master_key_process_environment_modes_are_covered_in_isolated_processes() {
    const MARKER: &str = "OPEN_COMPUTE_MASTER_KEY_CHILD_MODE";
    const KEY_ENV: &str = "OPEN_COMPUTE_MASTER_KEY_CHILD_VALUE";
    if let Ok(mode) = std::env::var(MARKER) {
        let (_tmp, root) = unique_root();
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(root.join("keys")).unwrap();
        fs::set_permissions(root.join("keys"), fs::Permissions::from_mode(0o700)).unwrap();
        let mut config = storage_config(&root);
        config.master_key_env = Some(KEY_ENV.to_owned());
        let result = master_key::inspect_existing(&config);
        match mode.as_str() {
            "valid" => assert_eq!(result.unwrap().bytes().expose(), &[0_u8; 32]),
            "empty" | "missing" | "invalid-utf8" => {
                assert_eq!(result.unwrap_err().code(), ErrorCode::MasterKeyMismatch);
            }
            _ => panic!("unexpected child mode"),
        }
        return;
    }

    use std::os::unix::ffi::OsStringExt;
    let current = std::env::current_exe().unwrap();
    for mode in ["valid", "empty", "missing", "invalid-utf8"] {
        let mut command = Command::new(&current);
        command.args([
            "--exact",
            "tests::master_key_process_environment_modes_are_covered_in_isolated_processes::master_key_process_environment_modes_are_covered_in_isolated_processes",
            "--test-threads=1",
        ]);
        command.env(MARKER, mode);
        match mode {
            "valid" => {
                command.env(KEY_ENV, "ocmk1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
            }
            "empty" => {
                command.env(KEY_ENV, "");
            }
            "missing" => {
                command.env_remove(KEY_ENV);
            }
            "invalid-utf8" => {
                command.env(KEY_ENV, std::ffi::OsString::from_vec(vec![0xff]));
            }
            _ => unreachable!(),
        }
        assert!(
            command.status().unwrap().success(),
            "child mode {mode} failed"
        );
    }
}
