//! Non-vacuous supply-chain and compiler tests.

use crate::compile::{
    CompileRequest, CompiledConfig, clear_after_config_rename_hook, compile_static_config,
    set_after_config_rename_hook,
};
use crate::digest::{
    DigestInputs, PlatformReleaseMeta, config_input_digest, digest_for, load_assets, render_config,
};
use crate::fsutil::{
    FILE_MODE, clear_publish_hook, set_publish_hook, set_test_max_asset_entries,
    set_test_max_asset_files, write_atomic_new,
};
use crate::lock::{RuntimeLock, load_runtime_lock};
use crate::process::{
    clear_exec_hook, clear_io_fail_hooks, clear_owner_reaped_hook, clear_owner_spawn_fail_hook,
    clear_pgid_verify_fail_hook, set_exec_hook, set_owner_reaped_hook, set_owner_spawn_fail_hook,
    set_pgid_verify_fail_hook, set_reader_panic, set_stdout_read_fail, set_stdout_write_fail,
    set_wait_fail_hook, wait_pid_gone, wait_reaped,
};
use crate::verify::verify_runtime_binary;
use open_compute_core::{ErrorCode, ReadinessReason, Redactor, SecretString};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

const VERSION: &str = "workerd 2026-08-30";
const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn sha256_file(path: &Path) -> String {
    let bytes = fs::read(path).expect("read");
    hex::encode(Sha256::digest(bytes))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn write_exec(path: &Path, body: &str) {
    let mut file = File::create(path).expect("create");
    file.write_all(body.as_bytes()).expect("write");
    let mut perms = file.metadata().expect("meta").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod");
}

async fn read_pid_file(path: &Path, timeout: Duration) -> i32 {
    let started = std::time::Instant::now();
    loop {
        match fs::read_to_string(path) {
            Ok(value) => match value.trim().parse() {
                Ok(pid) => return pid,
                Err(_) if started.elapsed() < timeout => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(error) => panic!("pid file must contain an integer: {error}"),
            },
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound && started.elapsed() < timeout =>
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => panic!("failed to read {}: {error}", path.display()),
        }
    }
}

fn host_target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", "x86_64") => "darwin-x64",
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-arm64",
        other => panic!("unsupported test host {other:?}"),
    }
}

fn archive_for_target(target: &str) -> &'static str {
    match target {
        "darwin-arm64" => "workerd-darwin-arm64.gz",
        "darwin-x64" => "workerd-darwin-64.gz",
        "linux-x64" => "workerd-linux-64.gz",
        "linux-arm64" => "workerd-linux-arm64.gz",
        other => panic!("unsupported test target {other}"),
    }
}

fn host_archive() -> &'static str {
    archive_for_target(host_target())
}

fn lock_json(binary_sha: &str, extra_target: &str) -> String {
    let target = host_target();
    let archive = host_archive();
    format!(
        r#"{{
  "schemaVersion": 4,
  "release": "v1.20260830.1",
  "revision": "e9dda5963aba7ee4323960db795690ec78fec118",
  "expectedVersionOutput": "{VERSION}",
  "systemCompatibilityDate": "2026-09-08",
  "systemCompatibilityFlags": ["experimental", "service_binding_extra_handlers"],
  "binaryMaximumCompatibilityDate": "2026-09-25",
  "compatibilityCatalog": {{
    "schemaVersion": 1,
    "sha256": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
  }},
  "processFlags": ["--experimental"],
  "pyodideBundle": {{
    "version": "314.0.6_2026-08-17_2",
    "fileName": "pyodide_314.0.6_2026-08-17_2.capnp.bin",
    "archiveName": "pyodide_314.0.6_2026-08-17_2.capnp.bin.gz",
    "archiveSha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    "bundleSha256": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
  }},
  "source": {{
    "repository": "https://github.com/elliothux/workerd",
    "upstreamBase": "dd8133e9b9656fb39f1434247a80aa7a249ee204",
    "buildInputs": {{ "bazel": "9.2.0", "target": "//src/workerd/server:workerd", "mode": "opt", "strip": "always", "macosExecRustStrip": "none" }}
  }},
  "workersTypes": {{
    "version": "5.20260830.1",
    "gitHead": "e9dda5963aba7ee4323960db795690ec78fec118",
    "packageSha256": "d3d7a80d3b27e53116e34736ec1945eb359f53a1000df37b205c4cb59ce29a8e",
    "astSha256": "a00b4783854c9028158f776d605790d9a3e17e6a97f4d255beb70035c59c40dd"
  }},
  "workersSdk": {{
    "revision": "f8085545bcaa2c639f171c25e4424685036a0e10",
    "cfVersion": "1.0.0-beta.12",
    "vitePluginVersion": "1.54.2"
  }},
  "targets": {{
    "{target}": {{
      "archiveName": "{archive}",
      "archiveUrl": "https://github.com/elliothux/workerd/releases/download/v1.20260830.1/{archive}",
      "archiveSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "binarySha256": "{binary_sha}"
    }}{extra_target}
  }}
}}"#
    )
}

fn write_lock(dir: &Path, binary_sha: &str) -> PathBuf {
    let path = dir.join("workerd.lock.json");
    fs::write(&path, lock_json(binary_sha, "")).expect("lock");
    path
}

fn version_script(counter: Option<&Path>) -> String {
    match counter {
        Some(path) => format!(
            "#!/bin/sh\nprintf x >> '{}'\necho '{VERSION}'\n",
            path.display()
        ),
        None => format!("#!/bin/sh\necho '{VERSION}'\n"),
    }
}

fn compile_script(counter: &Path, args_file: &Path, payload: &str, extra: &str) -> String {
    format!(
        "#!/bin/sh
printf x >> '{counter}'
printf '%s\\n' \"$0\" \"$@\" > '{args}'
if [ \"$1\" = \"--version\" ]; then
  echo '{VERSION}'
  exit 0
fi
{extra}
printf '%s' '{payload}'
",
        counter = counter.display(),
        args = args_file.display(),
        VERSION = VERSION,
        extra = extra,
        payload = payload,
    )
}

fn copy_formal_assets(dest: &Path) {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/runtime")
        .canonicalize()
        .expect("formal assets path");
    fs::create_dir_all(dest.join("dist")).expect("workers dir");
    fs::copy(src.join("config.capnp"), dest.join("config.capnp")).expect("config");
    for path in crate::fsutil::list_files_sorted(&src.join("dist")).expect("worker files") {
        let relative = path.strip_prefix(&src).expect("asset path");
        let output = dest.join(relative);
        fs::create_dir_all(output.parent().expect("asset parent")).expect("asset directory");
        fs::copy(path, output).expect("worker");
    }
}

fn redactor_with_token() -> Redactor {
    let mut r = Redactor::new();
    r.register_str(TOKEN);
    r
}

fn platform_meta() -> PlatformReleaseMeta {
    PlatformReleaseMeta {
        version: "0.1.0-test".into(),
    }
}

async fn verify_ok(lock_path: &Path, bin: &Path) -> crate::VerifiedRuntime {
    verify_runtime_binary(lock_path, bin, Duration::from_secs(5), &Redactor::new())
        .await
        .expect("verify")
}

fn compile_req<'a>(
    runtime: &'a crate::VerifiedRuntime,
    lock_path: &'a Path,
    assets: &'a Path,
    data: &'a Path,
    platform: &'a PlatformReleaseMeta,
    redactor: &'a Redactor,
    deadline: Duration,
) -> CompileRequest<'a> {
    CompileRequest {
        role: crate::ConfigRole::Runtime,
        runtime,
        lock_path,
        assets_dir: assets,
        runtime_data_dir: data,
        platform,
        ingress_socket: Path::new("/tmp/oc-runtime.sock"),
        durable_objects: open_compute_core::DurableObjectsConfig::default(),
        container_provider: None,
        deadline,
        redactor,
    }
}

fn pid_alive(pid: i32) -> bool {
    rustix::process::test_kill_process(rustix::process::Pid::from_raw(pid).unwrap()).is_ok()
}

fn leftover_names(data: &Path) -> Vec<std::ffi::OsString> {
    fs::read_dir(data)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect()
}

fn find_partial_config(data: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(data).ok()?;
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".partial.") {
            let candidate = entry.path().join("config.bin");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

mod lock_parse_rejects_unknown_schema_bad_url_hash_and_target;

mod load_lock_rejects_missing;

mod lock_validation_rejects_every_malformed_authority_field;

mod lock_target_url_identity_and_accessors_are_strict;

mod digest_assets_and_unix_addresses_are_fail_closed;
mod native_container_protocol;

mod missing_directory_rejected_before_version;

mod version_success_mismatch_nonzero_timeout_oversized_non_utf8;

mod real_pinned_binary_is_accepted;

mod supervisor_construction_debug_and_default_wiring_are_secret_safe;

mod asset_walk_bounds_zero_length_file_fanout;

mod input_digest_changes_with_any_input;

mod cache_reuse_and_corrupt_rebuild;

mod compile_failures_clean_partials;

mod cancel_compile_reaps_descendants_and_removes_work_dir;

mod concurrent_compiles_do_not_clobber_workspaces;

mod corrupt_sidecar_fails_closed;

mod real_compile_succeeds_when_env_set;

mod compiled_config_accessors_and_corruption_matrix;

mod packaged_lock_matches_formal_release_pin;

mod write_atomic_new_destination_appears_race_preserves_winner;

mod concurrent_same_digest_publish_reuses_one_winner;

mod same_digest_cache_lookup_cannot_delete_publish_window;

mod cancel_sends_term_before_process_is_gone;

mod fallback_does_not_signal_after_owner_reaps;

mod cancel_term_ignored_still_kills_descendants;

mod compile_stdout_streams_into_partial_before_exit;

mod descendant_holding_pipes_returns_within_deadline;

mod compile_stdout_write_failure_is_typed_and_cleans_up;

mod compile_stdout_read_failure_is_typed_and_cleans_up;

mod owner_thread_spawn_failure_kills_spawned_child;

mod pgid_verify_failure_kills_spawned_child;

mod wait_failure_drop_still_reaps_group;

mod reader_panic_is_typed_and_cleans_up;

#[path = "lock_source_tests.rs"]
mod lock_source;
