use super::*;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

const CASE_ENV: &str = "OPEN_COMPUTE_SERVICE_ACCOUNT_TEST_CASE";

fn run_child(case: &str, configure: impl FnOnce(&mut Command)) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "service_manager::env_tests::system_service_account_child",
        ])
        .env(CASE_ENV, case)
        .env_remove("SUDO_USER")
        .env_remove("SUDO_UID")
        .env_remove("SUDO_GID");
    configure(&mut command);
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "case {case} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn system_service_account_child() {
    let Ok(case) = std::env::var(CASE_ENV) else {
        return;
    };
    match case.as_str() {
        "fallback" | "success" => {
            let account = system_service_account().unwrap();
            assert_ne!(account.name, "root");
            assert_ne!(account.uid, 0);
            assert_ne!(account.gid, 0);
        }
        expected => {
            let error = system_service_account().unwrap_err();
            assert!(error.message().contains(expected), "{error:?}");
        }
    }
}

#[test]
fn system_service_account_validates_sudo_identity_in_isolated_processes() {
    let user = String::from_utf8(Command::new("id").arg("-un").output().unwrap().stdout)
        .unwrap()
        .trim()
        .to_owned();
    let uid = rustix::process::getuid().as_raw();
    let gid = rustix::process::getgid().as_raw();
    assert_ne!(user, "root");
    assert_ne!(uid, 0);
    assert_ne!(gid, 0);

    run_child("fallback", |_| {});
    run_child("non-root", |command| {
        command
            .env("SUDO_USER", "root")
            .env("SUDO_UID", "0")
            .env("SUDO_GID", "0");
    });
    run_child("validated SUDO_USER", |command| {
        command
            .env("SUDO_USER", &user)
            .env("SUDO_UID", "invalid")
            .env("SUDO_GID", gid.to_string());
    });
    run_child("does not exist", |command| {
        command
            .env("SUDO_USER", "open-compute-no-such-service-account")
            .env("SUDO_UID", uid.to_string())
            .env("SUDO_GID", gid.to_string());
    });
    run_child("does not match", |command| {
        command
            .env("SUDO_USER", &user)
            .env("SUDO_UID", uid.saturating_add(1).to_string())
            .env("SUDO_GID", gid.to_string());
    });
    run_child("success", |command| {
        command
            .env("SUDO_USER", &user)
            .env("SUDO_UID", uid.to_string())
            .env("SUDO_GID", gid.to_string());
    });

    let temp = tempfile::TempDir::new().unwrap();
    let fake_id = temp.path().join("id");
    fs::write(&fake_id, b"#!/bin/sh\nprintf 'not-an-id\\n'\n").unwrap();
    fs::set_permissions(&fake_id, fs::Permissions::from_mode(0o700)).unwrap();
    run_child("service account ID is invalid", |command| {
        command
            .env("PATH", temp.path())
            .env("SUDO_USER", &user)
            .env("SUDO_UID", uid.to_string())
            .env("SUDO_GID", gid.to_string());
    });
    run_child("failed to query service account", |command| {
        command
            .env("PATH", temp.path().join("missing"))
            .env("SUDO_USER", &user)
            .env("SUDO_UID", uid.to_string())
            .env("SUDO_GID", gid.to_string());
    });
}
