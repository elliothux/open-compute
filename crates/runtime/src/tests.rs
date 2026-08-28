//! Non-vacuous supply-chain and compiler tests.

use crate::compile::{
    CompileRequest, CompiledConfig, clear_after_config_rename_hook, compile_static_config,
    set_after_config_rename_hook,
};
use crate::digest::{
    BINDING_TOKEN_PLACEHOLDER, DigestInputs, PlatformReleaseMeta, config_input_digest, digest_for,
    load_assets, render_config, render_config_with_tokens, validate_token,
};
use crate::fetch::{PackageReleaseRequest, install_official_release, package_release_bundle};
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
use crate::verify::{clear_hash_cache, verify_runtime_binary};
use flate2::Compression;
use flate2::write::GzEncoder;
use open_compute_core::{ErrorCode, ReadinessReason, Redactor, SecretString};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

const VERSION: &str = "workerd 2026-08-26";
const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TOKEN_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

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
  "schemaVersion": 1,
  "release": "v1.20260826.1",
  "expectedVersionOutput": "{VERSION}",
  "hostCompatibilityDate": "2026-08-22",
  "processFlags": ["--experimental"],
  "hostCompatibilityFlags": ["nodejs_compat", "rpc", "enable_ctx_exports", "experimental"],
  "targets": {{
    "{target}": {{
      "archiveName": "{archive}",
      "archiveUrl": "https://github.com/cloudflare/workerd/releases/download/v1.20260826.1/{archive}",
      "archiveSha256": "22657ec7045a3677b7f52e97f106fe0493add57810687e755e8c6f4fba4b1dba",
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
        .join("../../runtime")
        .canonicalize()
        .expect("formal assets path");
    fs::create_dir_all(dest.join("system-workers")).expect("workers dir");
    fs::copy(src.join("config.capnp"), dest.join("config.capnp")).expect("config");
    for path in crate::fsutil::list_files_sorted(&src.join("system-workers")).expect("worker files")
    {
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
    clear_hash_cache();
    verify_runtime_binary(lock_path, bin, Duration::from_secs(5), &Redactor::new())
        .await
        .expect("verify")
}

#[allow(clippy::too_many_arguments)]
fn compile_req<'a>(
    runtime: &'a crate::VerifiedRuntime,
    lock_path: &'a Path,
    assets: &'a Path,
    data: &'a Path,
    platform: &'a PlatformReleaseMeta,
    token: &'a SecretString,
    redactor: &'a Redactor,
    deadline: Duration,
) -> CompileRequest<'a> {
    CompileRequest {
        runtime,
        lock_path,
        assets_dir: assets,
        runtime_data_dir: data,
        platform,
        token,
        binding_token: token,
        durable_objects: open_compute_core::DurableObjectsConfig::default(),
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

#[test]
fn lock_parse_rejects_unknown_schema_bad_url_hash_and_target() {
    let good = lock_json(&"ab".repeat(32), "");
    RuntimeLock::parse(good.as_bytes()).expect("good lock");

    let unknown = good.replace("\"schemaVersion\": 1", "\"schemaVersion\": 2");
    let err = RuntimeLock::parse(unknown.as_bytes()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::RuntimeInvalid);

    let extra_field = good.replacen('{', "{\"nope\":1,", 1);
    assert!(RuntimeLock::parse(extra_field.as_bytes()).is_err());

    let bad_url = good.replace("https://github.com/", "http://example.com/");
    assert!(RuntimeLock::parse(bad_url.as_bytes()).is_err());

    let bad_hash = lock_json("zzzz", "");
    assert!(RuntimeLock::parse(bad_hash.as_bytes()).is_err());

    let bad_target = lock_json(
        &"ab".repeat(32),
        r#",
    "solaris-sparc": {
      "archiveName": "x.gz",
      "archiveUrl": "https://github.com/cloudflare/workerd/releases/download/v1.20260826.1/x.gz",
      "archiveSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "binarySha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    }"#,
    );
    assert!(RuntimeLock::parse(bad_target.as_bytes()).is_err());

    let dup_keys = good.replacen(
        "\"schemaVersion\": 1",
        "\"schemaVersion\": 1, \"schemaVersion\": 1",
        1,
    );
    assert!(RuntimeLock::parse(dup_keys.as_bytes()).is_err());

    let bad_date = good.replace("2026-08-22", "2026-02-30");
    assert!(RuntimeLock::parse(bad_date.as_bytes()).is_err());

    let dup_flag = good.replace(
        "[\"--experimental\"]",
        "[\"--experimental\", \"--experimental\"]",
    );
    assert!(RuntimeLock::parse(dup_flag.as_bytes()).is_err());

    let archive = host_archive();
    let bad_archive = good.replacen(
        &format!("\"archiveName\": \"{archive}\""),
        "\"archiveName\": \"other.gz\"",
        1,
    );
    assert!(RuntimeLock::parse(bad_archive.as_bytes()).is_err());

    let foreign = if host_target() == "linux-x64" {
        "darwin-arm64"
    } else {
        "linux-x64"
    };
    let foreign_archive = archive_for_target(foreign);
    let host_named_foreign = good
        .replace(
            &format!("\"archiveName\": \"{archive}\""),
            &format!("\"archiveName\": \"{foreign_archive}\""),
        )
        .replace(
            &format!(
                "https://github.com/cloudflare/workerd/releases/download/v1.20260826.1/{archive}"
            ),
            &format!(
                "https://github.com/cloudflare/workerd/releases/download/v1.20260826.1/{foreign_archive}"
            ),
        );
    assert!(
        RuntimeLock::parse(host_named_foreign.as_bytes()).is_err(),
        "archiveName must be workerd-<target-key>.gz even when the URL uses that foreign name"
    );

    let extra_mismatch = format!(
        r#",
    "{foreign}": {{
      "archiveName": "{archive}",
      "archiveUrl": "https://github.com/cloudflare/workerd/releases/download/v1.20260826.1/{archive}",
      "archiveSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "binarySha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    }}"#
    );
    assert!(
        RuntimeLock::parse(lock_json(&"ab".repeat(32), &extra_mismatch).as_bytes()).is_err(),
        "a second target must not reuse another target's official archive name"
    );
}

#[test]
fn load_lock_rejects_symlink_and_missing() {
    let dir = TempDir::new().unwrap();
    let path = write_lock(dir.path(), &"ab".repeat(32));
    load_runtime_lock(&path).expect("regular lock");

    let link = dir.path().join("lock.link");
    symlink(&path, &link).unwrap();
    assert_eq!(
        load_runtime_lock(&link).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        load_runtime_lock(&dir.path().join("missing.json"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
}

#[test]
fn lock_validation_rejects_every_malformed_authority_field() {
    let good = lock_json(&"ab".repeat(32), "");
    let replacements = [
        ("\"release\": \"v1.20260826.1\"", "\"release\": \"\""),
        (
            "\"release\": \"v1.20260826.1\"",
            "\"release\": \" v1.20260826.1\"",
        ),
        (
            "\"expectedVersionOutput\": \"workerd 2026-08-26\"",
            "\"expectedVersionOutput\": \"\"",
        ),
        (
            "\"expectedVersionOutput\": \"workerd 2026-08-26\"",
            "\"expectedVersionOutput\": \"workerd 2026-08-26 \"",
        ),
        (
            "\"hostCompatibilityDate\": \"2026-08-22\"",
            "\"hostCompatibilityDate\": \"20260822\"",
        ),
        (
            "\"hostCompatibilityDate\": \"2026-08-22\"",
            "\"hostCompatibilityDate\": \"1969-01-01\"",
        ),
        (
            "\"hostCompatibilityDate\": \"2026-08-22\"",
            "\"hostCompatibilityDate\": \"2026-00-01\"",
        ),
        (
            "\"hostCompatibilityDate\": \"2026-08-22\"",
            "\"hostCompatibilityDate\": \"2026-13-01\"",
        ),
        (
            "\"hostCompatibilityDate\": \"2026-08-22\"",
            "\"hostCompatibilityDate\": \"2026-01-00\"",
        ),
        (
            "\"hostCompatibilityDate\": \"2026-08-22\"",
            "\"hostCompatibilityDate\": \"2100-02-29\"",
        ),
        (
            "\"processFlags\": [\"--experimental\"]",
            "\"processFlags\": []",
        ),
        (
            "\"processFlags\": [\"--experimental\"]",
            "\"processFlags\": [\"-x\"]",
        ),
        (
            "\"processFlags\": [\"--experimental\"]",
            "\"processFlags\": [\"--\"]",
        ),
        (
            "\"processFlags\": [\"--experimental\"]",
            "\"processFlags\": [\"--x=y\"]",
        ),
        (
            "\"processFlags\": [\"--experimental\"]",
            "\"processFlags\": [\"--x y\"]",
        ),
        (
            "\"hostCompatibilityFlags\": [\"nodejs_compat\", \"rpc\", \"enable_ctx_exports\", \"experimental\"]",
            "\"hostCompatibilityFlags\": []",
        ),
        (
            "\"hostCompatibilityFlags\": [\"nodejs_compat\", \"rpc\", \"enable_ctx_exports\", \"experimental\"]",
            "\"hostCompatibilityFlags\": [\"\"]",
        ),
        (
            "\"hostCompatibilityFlags\": [\"nodejs_compat\", \"rpc\", \"enable_ctx_exports\", \"experimental\"]",
            "\"hostCompatibilityFlags\": [\"node-js\"]",
        ),
        (
            "\"hostCompatibilityFlags\": [\"nodejs_compat\", \"rpc\", \"enable_ctx_exports\", \"experimental\"]",
            "\"hostCompatibilityFlags\": [\"rpc\", \"rpc\"]",
        ),
    ];
    for (needle, replacement) in replacements {
        let bad = good.replacen(needle, replacement, 1);
        assert_ne!(bad, good, "test replacement must match: {needle}");
        assert_eq!(
            RuntimeLock::parse(bad.as_bytes()).unwrap_err().code(),
            ErrorCode::RuntimeInvalid,
            "malformed lock unexpectedly accepted: {replacement}"
        );
    }

    let mut value: serde_json::Value = serde_json::from_str(&good).unwrap();
    value["targets"] = serde_json::json!({});
    assert!(RuntimeLock::parse(&serde_json::to_vec(&value).unwrap()).is_err());

    for scalar in ["true", "-1", "1", "1.5", "\"lock\"", "null", "[]"] {
        assert!(RuntimeLock::parse(scalar.as_bytes()).is_err());
    }
    assert!(RuntimeLock::parse(&vec![b' '; 1024 * 1024 + 1]).is_err());
}

#[test]
fn lock_target_url_identity_and_accessors_are_strict() {
    let good = lock_json(&"ab".repeat(32), "");
    let archive = host_archive();
    let url =
        format!("https://github.com/cloudflare/workerd/releases/download/v1.20260826.1/{archive}");
    let bad_urls = [
        "not a url".to_owned(),
        url.replacen("https://", "http://", 1),
        url.replacen("https://", "https://user:pass@", 1),
        url.replace("github.com", "example.com"),
        format!("{url}?download=1"),
        format!("{url}#fragment"),
        url.replace("v1.20260826.1", "v1.other"),
    ];
    for bad_url in bad_urls {
        let bad = good.replacen(&url, &bad_url, 1);
        assert!(
            RuntimeLock::parse(bad.as_bytes()).is_err(),
            "malformed archive URL unexpectedly accepted: {bad_url}"
        );
    }
    for bad_name in ["", "sub/workerd.gz", "sub\\workerd.gz"] {
        let bad = good.replacen(
            &format!("\"archiveName\": \"{archive}\""),
            &format!("\"archiveName\": \"{bad_name}\""),
            1,
        );
        assert!(RuntimeLock::parse(bad.as_bytes()).is_err());
    }
    let bad_archive_hash = good.replacen(
        "22657ec7045a3677b7f52e97f106fe0493add57810687e755e8c6f4fba4b1dba",
        "zz",
        1,
    );
    assert!(RuntimeLock::parse(bad_archive_hash.as_bytes()).is_err());

    let lock = RuntimeLock::parse(good.as_bytes()).unwrap();
    let (name, target) = lock.current_target().unwrap();
    assert_eq!(name, host_target());
    let debug = format!("{target:?}");
    assert!(debug.contains(&target.archive_name));
    assert_eq!(
        RuntimeLock::token_placeholder(),
        "__OPEN_COMPUTE_INTERNAL_TOKEN__"
    );

    let dir = TempDir::new().unwrap();
    assert_eq!(
        load_runtime_lock(Path::new("relative.lock"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        load_runtime_lock(dir.path()).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
}

#[test]
fn digest_assets_tokens_and_supervisor_auth_are_fail_closed() {
    assert_eq!(
        load_assets(Path::new("relative-assets"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("system-workers")).unwrap();
    fs::write(dir.path().join("config.capnp"), b"template").unwrap();
    assert_eq!(
        load_assets(dir.path()).unwrap_err().code(),
        ErrorCode::ConfigCompileFailed
    );
    fs::write(
        dir.path().join("system-workers/worker.js"),
        b"export default {}",
    )
    .unwrap();
    let (_, workers, config_path) = load_assets(dir.path()).unwrap();
    assert_eq!(workers[0].0, "system-workers/worker.js");
    assert_eq!(config_path, dir.path().join("config.capnp"));

    let valid = SecretString::new(TOKEN);
    assert_eq!(
        render_config(RuntimeLock::token_placeholder(), &valid).unwrap(),
        TOKEN
    );
    let binding = SecretString::new(TOKEN_B);
    let rendered = render_config_with_tokens(
        &format!(
            "{}:{}",
            RuntimeLock::token_placeholder(),
            BINDING_TOKEN_PLACEHOLDER
        ),
        &valid,
        &binding,
    )
    .unwrap();
    assert_eq!(rendered, format!("{TOKEN}:{TOKEN_B}"));
    assert_eq!(
        render_config_with_tokens(
            &format!(
                "{}:{}",
                RuntimeLock::token_placeholder(),
                BINDING_TOKEN_PLACEHOLDER
            ),
            &valid,
            &valid,
        )
        .unwrap_err()
        .code(),
        ErrorCode::RuntimeInvalid
    );
    for token in [
        SecretString::new("short"),
        SecretString::new("g".repeat(64)),
        SecretString::new("A".repeat(64)),
    ] {
        assert_eq!(
            validate_token(&token).unwrap_err().code(),
            ErrorCode::RuntimeInvalid
        );
    }
    assert_eq!(
        render_config("no placeholder", &valid).unwrap_err().code(),
        ErrorCode::ConfigCompileFailed
    );
    assert_eq!(
        render_config(
            &format!(
                "{}{}",
                RuntimeLock::token_placeholder(),
                RuntimeLock::token_placeholder()
            ),
            &valid,
        )
        .unwrap_err()
        .code(),
        ErrorCode::ConfigCompileFailed
    );

    use crate::supervisor::{
        ExternalServiceAddress, GenerationAuthRegistry, SupervisorSnapshot, SupervisorState,
        generate_internal_token, token_fingerprint,
    };
    assert!(
        ExternalServiceAddress::loopback("runtime-source", "127.0.0.1:8080".parse().unwrap())
            .is_ok()
    );
    for (name, address) in [
        ("", "127.0.0.1:8080"),
        (&"x".repeat(65), "127.0.0.1:8080"),
        ("bad name", "127.0.0.1:8080"),
        ("valid", "127.0.0.1:0"),
        ("valid", "192.0.2.1:8080"),
    ] {
        assert!(ExternalServiceAddress::loopback(name, address.parse().unwrap()).is_err());
    }
    let states = [
        (SupervisorState::Stopped, "STOPPED"),
        (SupervisorState::Starting, "STARTING"),
        (SupervisorState::Running, "RUNNING"),
        (SupervisorState::BackingOff, "BACKING_OFF"),
        (SupervisorState::Failed, "FAILED"),
        (SupervisorState::Draining, "DRAINING"),
        (SupervisorState::Stopping, "STOPPING"),
    ];
    for (state, expected) in states {
        assert_eq!(state.as_str(), expected);
    }
    let snapshot = SupervisorSnapshot::initial(SystemTime::UNIX_EPOCH, "digest".into());
    assert_eq!(snapshot.state, SupervisorState::Stopped);
    assert_eq!(snapshot.reason, ReadinessReason::Starting);
    assert!(!format!("{snapshot:?}").contains("listen_port"));

    let auth = GenerationAuthRegistry::new();
    assert!(!auth.authorize(TOKEN, "generation"));
    assert!(auth.credential().is_none());
    assert!(auth.active_fingerprint().is_none());
    auth.activate(valid.clone());
    let credential = auth.credential().unwrap();
    assert_eq!(credential.expose(), TOKEN);
    assert_eq!(
        format!("{credential:?}"),
        "GenerationCredential([REDACTED])"
    );
    assert!(!format!("{auth:?}").contains(TOKEN));
    for generation in ["", "\n", &"x".repeat(129)] {
        assert!(!auth.authorize(TOKEN, generation));
    }
    assert!(!auth.authorize("bad", "generation"));
    assert!(!auth.authorize(TOKEN_B, "generation"));
    assert!(auth.authorize(TOKEN, "generation"));
    assert!(auth.authorize(TOKEN, "generation"));
    assert!(!auth.authorize(TOKEN, "other"));
    assert_eq!(auth.active_fingerprint(), Some(token_fingerprint(&valid)));
    auth.clear();
    assert!(auth.credential().is_none());

    let generated = generate_internal_token().unwrap();
    validate_token(&generated).unwrap();
    assert_eq!(token_fingerprint(&generated).len(), 16);
}

#[tokio::test]
async fn missing_symlink_directory_non_executable_tampered_rejected_before_version() {
    let dir = TempDir::new().unwrap();
    let counter = dir.path().join("ran");
    let bin = dir.path().join("workerd");
    write_exec(&bin, &version_script(Some(&counter)));
    let hash = sha256_file(&bin);
    let lock_path = write_lock(dir.path(), &hash);

    let missing = dir.path().join("nope");
    let err = verify_runtime_binary(
        &lock_path,
        &missing,
        Duration::from_secs(2),
        &Redactor::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::RuntimeInvalid);
    assert!(!counter.exists());

    let link = dir.path().join("workerd.link");
    symlink(&bin, &link).unwrap();
    assert!(
        verify_runtime_binary(&lock_path, &link, Duration::from_secs(2), &Redactor::new())
            .await
            .is_err()
    );
    assert!(!counter.exists());

    let as_dir = dir.path().join("workerd-dir");
    fs::create_dir(&as_dir).unwrap();
    assert!(
        verify_runtime_binary(
            &lock_path,
            &as_dir,
            Duration::from_secs(2),
            &Redactor::new()
        )
        .await
        .is_err()
    );
    assert!(!counter.exists());

    let non_exec = dir.path().join("workerd-ne");
    fs::copy(&bin, &non_exec).unwrap();
    let mut perms = fs::metadata(&non_exec).unwrap().permissions();
    perms.set_mode(0o644);
    fs::set_permissions(&non_exec, perms).unwrap();
    let ne_hash = sha256_file(&non_exec);
    let ne_lock_path = dir.path().join("ne.lock.json");
    fs::write(&ne_lock_path, lock_json(&ne_hash, "")).unwrap();
    assert!(
        verify_runtime_binary(
            &ne_lock_path,
            &non_exec,
            Duration::from_secs(2),
            &Redactor::new()
        )
        .await
        .is_err()
    );
    assert!(!counter.exists());

    let tampered = dir.path().join("workerd-bad");
    write_exec(
        &tampered,
        "#!/bin/sh\nprintf x >> /dev/null\necho 'workerd 2026-08-26'\n# tampered\n",
    );
    assert_eq!(
        verify_runtime_binary(
            &lock_path,
            &tampered,
            Duration::from_secs(2),
            &Redactor::new()
        )
        .await
        .unwrap_err()
        .code(),
        ErrorCode::RuntimeInvalid
    );
    assert!(!counter.exists(), "tampered binary must not be executed");
}

#[tokio::test]
async fn version_success_mismatch_nonzero_timeout_oversized_non_utf8() {
    let dir = TempDir::new().unwrap();

    let ok = dir.path().join("ok");
    write_exec(&ok, &version_script(None));
    let lock_path = write_lock(dir.path(), &sha256_file(&ok));
    let verified = verify_ok(&lock_path, &ok).await;
    assert_eq!(verified.version_output(), VERSION);
    assert_eq!(verified, verified.clone());
    let verified_debug = format!("{verified:?}");
    assert!(verified_debug.contains(verified.target()));
    assert!(verified_debug.contains(verified.release()));
    assert!(verified_debug.contains(verified.binary_sha256()));

    let mismatch = dir.path().join("mismatch");
    write_exec(&mismatch, "#!/bin/sh\necho 'workerd 1999-01-01'\n");
    let mlock = write_lock(dir.path(), &sha256_file(&mismatch));
    assert_eq!(
        verify_runtime_binary(&mlock, &mismatch, Duration::from_secs(2), &Redactor::new())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::RuntimeInvalid
    );

    let nonzero = dir.path().join("nonzero");
    write_exec(&nonzero, "#!/bin/sh\necho 'workerd 2026-08-26'\nexit 3\n");
    let nlock = dir.path().join("n.lock.json");
    fs::write(&nlock, lock_json(&sha256_file(&nonzero), "")).unwrap();
    assert!(
        verify_runtime_binary(&nlock, &nonzero, Duration::from_secs(2), &Redactor::new())
            .await
            .is_err()
    );

    let sleepy = dir.path().join("sleep");
    write_exec(&sleepy, "#!/bin/sh\nsleep 30\n");
    let slock = dir.path().join("s.lock.json");
    fs::write(&slock, lock_json(&sha256_file(&sleepy), "")).unwrap();
    let started = std::time::Instant::now();
    let err = verify_runtime_binary(
        &slock,
        &sleepy,
        Duration::from_millis(400),
        &Redactor::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::RuntimeInvalid);
    assert!(started.elapsed() < Duration::from_secs(5));

    let big = dir.path().join("big");
    write_exec(
        &big,
        "#!/bin/sh\nawk 'BEGIN{for(i=0;i<9000;i++)printf \"a\"}'\n",
    );
    let block = dir.path().join("b.lock.json");
    fs::write(&block, lock_json(&sha256_file(&big), "")).unwrap();
    assert!(
        verify_runtime_binary(&block, &big, Duration::from_secs(2), &Redactor::new())
            .await
            .is_err()
    );

    let binary = dir.path().join("binout");
    write_exec(&binary, "#!/bin/sh\nprintf '\\xff\\xfe'\n");
    let block2 = dir.path().join("b2.lock.json");
    fs::write(&block2, lock_json(&sha256_file(&binary), "")).unwrap();
    assert!(
        verify_runtime_binary(&block2, &binary, Duration::from_secs(2), &Redactor::new())
            .await
            .is_err()
    );
}

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

#[tokio::test]
async fn real_pinned_binary_accepted_when_env_set() {
    let Some(path) = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD") else {
        eprintln!("OPEN_COMPUTE_TEST_WORKERD unset; real workerd test not executed");
        return;
    };
    let path = PathBuf::from(path);
    assert!(path.is_absolute());
    let lock_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/workerd.lock.json");
    let verified = verify_runtime_binary(
        &lock_path.canonicalize().unwrap(),
        &path,
        Duration::from_secs(10),
        &Redactor::new(),
    )
    .await
    .expect("real workerd must verify");
    assert_eq!(verified.version_output(), VERSION);
    assert_eq!(verified.release(), "v1.20260826.1");
}

#[tokio::test]
async fn supervisor_construction_debug_and_default_wiring_are_secret_safe() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let binary = dir.path().join("workerd");
    write_exec(&binary, &version_script(None));
    let lock_path = write_lock(dir.path(), &sha256_file(&binary));
    let runtime = verify_ok(&lock_path, &binary).await;
    let data = dir.path().join("runtime-data");
    fs::create_dir(&data).unwrap();
    let compiler = crate::StaticConfigCompiler::new(
        runtime.clone(),
        lock_path,
        dir.path().to_path_buf(),
        data,
        platform_meta(),
        Duration::from_secs(1),
        Redactor::new(),
    );
    assert!(format!("{compiler:?}").contains("StaticConfigCompiler"));
    assert!(format!("{:?}", crate::FnCompiler(())).contains("FnCompiler"));

    let options = crate::WorkerdSupervisorOptions {
        runtime: runtime.clone(),
        compiler: compiler.clone(),
        config: open_compute_core::config::RuntimeConfig::default(),
        clock: Arc::new(open_compute_core::SystemClock),
        jitter: Arc::new(crate::OsJitter),
        redactor: Redactor::new(),
        lease_path: None,
    };
    assert!(format!("{options:?}").contains("WorkerdSupervisorOptions"));
    let supervisor = crate::WorkerdSupervisor::new(options);
    assert!(format!("{supervisor:?}").contains("WorkerdSupervisor"));
    supervisor.shutdown().await;

    let defaults = crate::WorkerdSupervisor::with_defaults(
        runtime,
        compiler,
        open_compute_core::config::RuntimeConfig::default(),
        Redactor::new(),
    );
    assert!(format!("{defaults:?}").contains("WorkerdSupervisor"));
    defaults.shutdown().await;
}

#[test]
fn asset_walk_bounds_zero_length_file_fanout() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("system-workers")).unwrap();
    fs::write(dir.path().join("config.capnp"), b"template").unwrap();
    for i in 0..3 {
        fs::write(
            dir.path().join("system-workers").join(format!("w{i}.js")),
            b"",
        )
        .unwrap();
    }
    set_test_max_asset_files(Some(2));
    let err = load_assets(dir.path()).unwrap_err();
    set_test_max_asset_files(None);
    assert_eq!(err.code(), ErrorCode::PathInvalid);

    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("system-workers")).unwrap();
    fs::write(dir.path().join("config.capnp"), b"template").unwrap();
    fs::create_dir(dir.path().join("system-workers").join("a")).unwrap();
    fs::create_dir(dir.path().join("system-workers").join("b")).unwrap();
    set_test_max_asset_entries(Some(1));
    let err = load_assets(dir.path()).unwrap_err();
    set_test_max_asset_entries(None);
    assert_eq!(err.code(), ErrorCode::PathInvalid);

    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("system-workers")).unwrap();
    fs::write(dir.path().join("config.capnp"), b"template").unwrap();
    for i in 0..9 {
        fs::write(
            dir.path().join("system-workers").join(format!("w{i}.js")),
            vec![b'x'; 1024 * 1024],
        )
        .unwrap();
    }
    assert_eq!(
        load_assets(dir.path()).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
}

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
    let lock = runtime.lock().clone();
    let token = SecretString::new(TOKEN);

    fs::write(dir.path().join("config.capnp"), [0xff]).unwrap();
    assert_eq!(
        digest_for(
            dir.path(),
            &lock,
            &lock_bytes,
            &runtime,
            &platform_meta(),
            &token,
        )
        .unwrap_err()
        .code(),
        ErrorCode::ConfigCompileFailed
    );
    copy_formal_assets(dir.path());
    let (d1, _, _) = digest_for(
        dir.path(),
        &lock,
        &lock_bytes,
        &runtime,
        &platform_meta(),
        &token,
    )
    .unwrap();
    let (d2, _, _) = digest_for(
        dir.path(),
        &lock,
        &lock_bytes,
        &runtime,
        &platform_meta(),
        &token,
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
        &lock,
        &lock_bytes,
        &runtime,
        &platform_meta(),
        &token,
    )
    .unwrap();
    assert_ne!(d1, d_cfg);
    copy_formal_assets(dir.path());

    fs::write(
        dir.path().join("system-workers/gateway/ingress.js"),
        fs::read(dir.path().join("system-workers/gateway/ingress.js")).unwrap(),
    )
    .unwrap();
    let extra = dir.path().join("system-workers/extra.js");
    fs::write(
        &extra,
        b"export default {fetch(){return new Response('x')}}",
    )
    .unwrap();
    let (d_w, _, _) = digest_for(
        dir.path(),
        &lock,
        &lock_bytes,
        &runtime,
        &platform_meta(),
        &token,
    )
    .unwrap();
    assert_ne!(d1, d_w);
    fs::remove_file(&extra).unwrap();

    let mut lock_bytes2 = lock_bytes.clone();
    lock_bytes2.extend_from_slice(b" ");
    let (d_lock, rendered, workers) = digest_for(
        dir.path(),
        &lock,
        &lock_bytes,
        &runtime,
        &platform_meta(),
        &token,
    )
    .unwrap();
    let d_lock2 = config_input_digest(&DigestInputs {
        config_template: &fs::read(dir.path().join("config.capnp")).unwrap(),
        workers: &workers,
        lock_bytes: &lock_bytes2,
        runtime: &runtime,
        platform: &platform_meta(),
        rendered: rendered.as_bytes(),
    });
    assert_ne!(d_lock, d_lock2);

    let runtime2 = runtime.clone().with_binary_sha256("cd".repeat(32));
    let (d_bin, _, _) = digest_for(
        dir.path(),
        &lock,
        &lock_bytes,
        &runtime2,
        &platform_meta(),
        &token,
    )
    .unwrap();
    assert_ne!(d1, d_bin);

    let other = SecretString::new(TOKEN_B);
    let (d_tok, _, _) = digest_for(
        dir.path(),
        &lock,
        &lock_bytes,
        &runtime,
        &platform_meta(),
        &other,
    )
    .unwrap();
    assert_ne!(d1, d_tok);

    let meta2 = PlatformReleaseMeta {
        version: "other".into(),
    };
    let (d_rel, _, _) =
        digest_for(dir.path(), &lock, &lock_bytes, &runtime, &meta2, &token).unwrap();
    assert_ne!(d1, d_rel);
}

#[tokio::test]
async fn cache_reuse_and_corrupt_rebuild() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let counter = dir.path().join("count");
    let args = dir.path().join("args");
    let bin = dir.path().join("workerd");
    write_exec(&bin, &compile_script(&counter, &args, "COMPILED-BYTES", ""));
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let mut redactor = redactor_with_token();
    redactor.register_secret_string(&token);
    let platform = platform_meta();

    let first_request = compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    );
    let request_debug = format!("{first_request:?}");
    assert!(request_debug.contains("CompileRequest"));
    assert!(!request_debug.contains(TOKEN));
    let first = compile_static_config(first_request).await.expect("compile");
    let n1 = fs::read(&counter).unwrap().len();
    assert!(n1 >= 1);
    let debug = format!("{first:?}");
    assert!(!debug.contains(TOKEN));
    assert!(!debug.contains(first.path().to_string_lossy().as_ref()));
    let args_text = fs::read_to_string(&args).unwrap();
    assert!(!args_text.contains(TOKEN));
    first.open().expect("revalidate compiled handle");

    let _second = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    ))
    .await
    .expect("reuse");
    let n2 = fs::read(&counter).unwrap().len();
    assert_eq!(n2, n1, "valid cache must not spawn compiler");

    fs::write(first.path(), b"corrupt").unwrap();
    assert!(first.open().is_err());
    let _third = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    ))
    .await
    .expect("rebuild");
    let n3 = fs::read(&counter).unwrap().len();
    assert!(n3 > n2);

    let mut perms = fs::metadata(first.path()).unwrap().permissions();
    perms.set_mode(0o644);
    fs::set_permissions(first.path(), perms).unwrap();
    let _ = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    ))
    .await
    .expect("rebuild after mode");

    let dest = first.path().to_path_buf();
    fs::remove_file(&dest).ok();
    symlink(dir.path().join("workerd"), &dest).ok();
    let rebuilt = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    ))
    .await
    .expect("rebuild after symlink");
    assert!(rebuilt.path().is_file());
}

#[tokio::test]
async fn compile_failures_clean_partials() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let counter = dir.path().join("count");
    let args = dir.path().join("args");
    let bin = dir.path().join("workerd");
    write_exec(&bin, &compile_script(&counter, &args, "", "exit 9"));
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let redactor = redactor_with_token();
    let platform = platform_meta();
    let err = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    ))
    .await
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigCompileFailed);
    let leftovers: Vec<_> = fs::read_dir(&data)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect();
    assert!(
        leftovers
            .iter()
            .all(|n| !n.to_string_lossy().contains("partial")
                && !n.to_string_lossy().contains("compile")),
        "partials must be removed: {leftovers:?}"
    );

    let sleepy = dir.path().join("sleepy");
    write_exec(
        &sleepy,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'workerd 2026-08-26'; exit 0; fi\nsleep 30\n",
    );
    let slock_path = dir.path().join("sleepy.lock.json");
    fs::write(&slock_path, lock_json(&sha256_file(&sleepy), "")).unwrap();
    let srt = verify_ok(&slock_path, &sleepy).await;
    let err = compile_static_config(compile_req(
        &srt,
        &slock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_millis(300),
    ))
    .await
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigCompileFailed);

    let big = dir.path().join("bigc");
    write_exec(
        &big,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'workerd 2026-08-26'; exit 0; fi\ndd if=/dev/zero bs=1048576 count=18 2>/dev/null\n",
    );
    let block = dir.path().join("big.lock.json");
    fs::write(&block, lock_json(&sha256_file(&big), "")).unwrap();
    let brt = verify_ok(&block, &big).await;
    let err = compile_static_config(compile_req(
        &brt,
        &block,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    ))
    .await
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigCompileFailed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_compile_reaps_descendants_and_removes_work_dir() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let pid_file = dir.path().join("pid");
    let child_file = dir.path().join("child");
    let bin = dir.path().join("workerd");
    write_exec(
        &bin,
        &format!(
            "#!/bin/sh
if [ \"$1\" = \"--version\" ]; then echo '{VERSION}'; exit 0; fi
echo $$ > '{pid}'
sleep 30 &
echo $! > '{child}'
wait
",
            pid = pid_file.display(),
            child = child_file.display(),
        ),
    );
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let redactor = redactor_with_token();
    let platform = platform_meta();
    let mut fut = Box::pin(compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(30),
    )));
    let wait_pid = async {
        let started = std::time::Instant::now();
        while !pid_file.exists() && started.elapsed() < Duration::from_secs(2) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let pid: i32 = fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        while !child_file.exists() && started.elapsed() < Duration::from_secs(2) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let child: i32 = fs::read_to_string(&child_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        (pid, child)
    };
    let (pid, child) = tokio::select! {
        _ = fut.as_mut() => panic!("compile finished before cancellation"),
        ids = wait_pid => ids,
    };
    drop(fut);
    tokio::time::sleep(Duration::from_millis(50)).await;
    wait_reaped(pid, Duration::from_secs(4)).expect("parent pid reaped");
    wait_pid_gone(child, Duration::from_secs(4)).expect("descendant reaped");
    assert!(!pid_alive(pid));
    assert!(!pid_alive(child));
    let leftovers: Vec<_> = fs::read_dir(&data)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect();
    assert!(
        leftovers.is_empty(),
        "work directories must be removed on cancel: {leftovers:?}"
    );
}

#[tokio::test]
async fn concurrent_compiles_do_not_clobber_workspaces() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let counter = dir.path().join("count");
    let args = dir.path().join("args");
    let bin = dir.path().join("workerd");
    write_exec(
        &bin,
        &compile_script(&counter, &args, "COMPILED-BYTES", "sleep 0.2"),
    );
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let redactor = redactor_with_token();
    let platform = platform_meta();
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
    let ca = ra.expect("first concurrent compile");
    let cb = rb.expect("second concurrent compile");
    assert_eq!(ca.digest(), cb.digest());
    ca.open().expect("winner must revalidate");
    cb.open().expect("winner must revalidate");
}

#[tokio::test]
async fn symlink_ancestor_assets_rejected() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let real = dir.path().join("real-assets");
    fs::rename(dir.path().join("config.capnp"), real.join("config.capnp")).ok();
    let assets = dir.path().join("assets");
    fs::create_dir_all(dir.path().join("target")).unwrap();
    copy_formal_assets(&dir.path().join("target"));
    symlink(dir.path().join("target"), &assets).unwrap();
    let bin = dir.path().join("workerd");
    write_exec(&bin, &version_script(None));
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let redactor = redactor_with_token();
    let platform = platform_meta();
    let err = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        &assets,
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    ))
    .await
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::PathInvalid);
}

#[tokio::test]
async fn corrupt_sidecar_fails_closed() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let counter = dir.path().join("count");
    let args = dir.path().join("args");
    let bin = dir.path().join("workerd");
    write_exec(&bin, &compile_script(&counter, &args, "COMPILED-BYTES", ""));
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
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
    .await
    .unwrap();
    let sidecar = compiled.path().with_extension("bin.digest");
    fs::write(&sidecar, b"nope\n").unwrap();
    assert!(compiled.open().is_err());
    let rebuilt = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    ))
    .await
    .expect("corrupt sidecar must rebuild, not skip");
    rebuilt.open().unwrap();
}

#[tokio::test]
async fn real_compile_succeeds_when_env_set() {
    let Some(path) = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD") else {
        eprintln!("OPEN_COMPUTE_TEST_WORKERD unset; real workerd test not executed");
        return;
    };
    let binary = PathBuf::from(path);
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtime")
        .canonicalize()
        .unwrap();
    let lock_path = assets.join("workerd.lock.json");
    let runtime = verify_runtime_binary(
        &lock_path,
        &binary,
        Duration::from_secs(10),
        &Redactor::new(),
    )
    .await
    .unwrap();
    let dir = TempDir::new().unwrap();
    let data = dir.path().join("runtime");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let mut redactor = Redactor::new();
    redactor.register_secret_string(&token);
    let platform = platform_meta();
    let compiled = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        &assets,
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(20),
    ))
    .await
    .expect("real compile");
    let mut file = compiled.open().unwrap();
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes).unwrap();
    assert!(!bytes.is_empty());
    assert!(!format!("{compiled:?}").contains(TOKEN));
    assert!(!compiled.to_string().contains(TOKEN));
    let err = open_compute_core::PlatformError::new(
        ErrorCode::ConfigCompileFailed,
        "workerd compile exited unsuccessfully",
    );
    assert!(!err.to_string().contains(TOKEN));
    assert!(!format!("{err:?}").contains(TOKEN));
}

#[test]
fn install_rejects_hash_mismatch_and_existing_dest() {
    let dir = TempDir::new().unwrap();
    let payload = b"not-a-real-binary";
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(payload).unwrap();
    let gz = encoder.finish().unwrap();
    let lock = RuntimeLock::parse(
        lock_json(&sha256_bytes(payload), "")
            .replace(
                "22657ec7045a3677b7f52e97f106fe0493add57810687e755e8c6f4fba4b1dba",
                &sha256_bytes(&gz),
            )
            .as_bytes(),
    );
    if let Ok(lock) = lock {
        let dest = dir.path().join("rel");
        let err = install_official_release(&lock, &dest, false, Some(b"wrong"));
        assert!(err.is_err());
        fs::create_dir(&dest).unwrap();
        assert!(install_official_release(&lock, &dest, false, Some(&gz)).is_err());
    }
}

#[test]
fn install_and_package_reject_failure_matrix_without_downloading() {
    fn release_lock(binary: &[u8], archive: &[u8]) -> RuntimeLock {
        let json = lock_json(&sha256_bytes(binary), "").replace(
            "22657ec7045a3677b7f52e97f106fe0493add57810687e755e8c6f4fba4b1dba",
            &sha256_bytes(archive),
        );
        RuntimeLock::parse(json.as_bytes()).unwrap()
    }

    let dir = TempDir::new().unwrap();
    let valid_script = version_script(None);
    let valid_gz = gzip_bytes(valid_script.as_bytes());
    let valid_lock = release_lock(valid_script.as_bytes(), &valid_gz);
    assert_eq!(
        install_official_release(&valid_lock, Path::new("relative"), false, Some(&valid_gz))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        install_official_release(&valid_lock, &dir.path().join("no-archive"), false, None)
            .unwrap_err()
            .code(),
        ErrorCode::RuntimeInvalid
    );

    let corrupt_gz = b"not a gzip stream";
    let corrupt_lock = release_lock(valid_script.as_bytes(), corrupt_gz);
    assert!(
        install_official_release(
            &corrupt_lock,
            &dir.path().join("corrupt-gzip"),
            false,
            Some(corrupt_gz)
        )
        .is_err()
    );

    let wrong_binary_lock = release_lock(b"different", &valid_gz);
    assert!(
        install_official_release(
            &wrong_binary_lock,
            &dir.path().join("wrong-binary"),
            false,
            Some(&valid_gz)
        )
        .is_err()
    );

    for (name, script) in [
        ("nonzero", "#!/bin/sh\nexit 7\n"),
        ("version-mismatch", "#!/bin/sh\necho 'workerd other'\n"),
        ("not-executable", "not an executable"),
    ] {
        let gz = gzip_bytes(script.as_bytes());
        let lock = release_lock(script.as_bytes(), &gz);
        assert!(
            install_official_release(&lock, &dir.path().join(name), false, Some(&gz)).is_err(),
            "installer unexpectedly accepted {name}"
        );
    }

    let platformd = dir.path().join("platformd");
    let assets = dir.path().join("assets");
    let license = dir.path().join("LICENSE");
    let default_config = dir.path().join("default.toml");
    let runbooks = dir.path().join("runbooks");
    let destination = dir.path().join("bundle");
    let mut request = PackageReleaseRequest {
        lock: &valid_lock,
        dest_dir: &destination,
        platformd: &platformd,
        assets_dir: &assets,
        license_file: &license,
        default_config: &default_config,
        runbooks_dir: &runbooks,
        release_json: b"{}",
        download: false,
        archive_bytes: Some(&valid_gz),
    };
    assert!(format!("{request:?}").contains("PackageReleaseRequest"));
    request.dest_dir = Path::new("relative");
    assert!(package_release_bundle(&request).is_err());
    request.dest_dir = &destination;
    request.platformd = Path::new("relative");
    assert!(package_release_bundle(&request).is_err());
    request.platformd = &platformd;
    request.assets_dir = Path::new("relative");
    assert!(package_release_bundle(&request).is_err());
    request.assets_dir = &assets;
    request.license_file = Path::new("relative");
    assert!(package_release_bundle(&request).is_err());
    request.license_file = &license;
    request.default_config = Path::new("relative");
    assert!(package_release_bundle(&request).is_err());
    request.default_config = &default_config;
    request.runbooks_dir = Path::new("relative");
    assert!(package_release_bundle(&request).is_err());
}

#[test]
fn compiled_config_accessors_and_corruption_matrix() {
    fn make(dir: &Path, name: &str) -> CompiledConfig {
        CompiledConfig::from_bytes_for_test(&dir.join(name), &"ab".repeat(32), b"compiled").unwrap()
    }

    let dir = TempDir::new().unwrap();
    let valid = make(dir.path(), "valid");
    assert_eq!(valid.digest(), "ab".repeat(32));
    assert!(valid.path().is_absolute());
    assert_eq!(valid.read_bytes().unwrap(), b"compiled");
    assert!(format!("{valid:?}").contains(valid.digest()));
    assert!(valid.to_string().contains(valid.digest()));

    for (name, sidecar_bytes, mode) in [
        ("empty", Vec::new(), 0o600),
        ("oversized", vec![b'x'; 257], 0o600),
        ("non-utf8", vec![0xff, 0xfe], 0o600),
        ("wrong-digest", b"wrong\nwrong\n".to_vec(), 0o600),
        (
            "wrong-content",
            format!("{}\n{}\n", "ab".repeat(32), "cd".repeat(32)).into_bytes(),
            0o600,
        ),
        (
            "wrong-mode",
            format!("{}\n{}\n", "ab".repeat(32), sha256_bytes(b"compiled")).into_bytes(),
            0o644,
        ),
    ] {
        let compiled = make(dir.path(), name);
        let sidecar = compiled.path().with_extension("bin.digest");
        fs::write(&sidecar, sidecar_bytes).unwrap();
        fs::set_permissions(&sidecar, fs::Permissions::from_mode(mode)).unwrap();
        assert!(compiled.open().is_err(), "corrupt sidecar accepted: {name}");
    }

    let empty = make(dir.path(), "empty-config");
    fs::write(empty.path(), b"").unwrap();
    assert!(empty.open().is_err());
    let wrong_mode = make(dir.path(), "wrong-config-mode");
    fs::set_permissions(wrong_mode.path(), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(wrong_mode.open().is_err());
}

#[test]
fn packaged_lock_matches_g0_pin() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/workerd.lock.json");
    let (lock, _) = load_runtime_lock(&path.canonicalize().unwrap()).unwrap();
    assert_eq!(lock.release, "v1.20260826.1");
    assert_eq!(lock.expected_version_output, VERSION);
    assert_eq!(lock.host_compatibility_date, "2026-08-22");
    assert_eq!(lock.process_flags, vec!["--experimental".to_string()]);
    let darwin = lock.targets.get("darwin-arm64").unwrap();
    assert_eq!(
        darwin.archive_sha256,
        "22657ec7045a3677b7f52e97f106fe0493add57810687e755e8c6f4fba4b1dba"
    );
    assert_eq!(
        darwin.binary_sha256,
        "2d17da54d2671d6e9e7c776d56b934f60be8c140b9bac35ddf22f60d6cff9403"
    );
    let expected = [
        (
            "darwin-x64",
            "61b644abde08329d3057634e591bd72a9cd5adc3424edd66509b138648289e37",
            "b1046219d7b5b5e86047f44cb3372b803741a772db632a54ba987ee0f16dcd58",
        ),
        (
            "linux-x64",
            "b832c71df79585b7eb361205f531aeebd6b4f15a0934ecdbfdf01d32c025ed63",
            "32976646cded43835d624c138d10121f63a692e47df0438390ab11a072345880",
        ),
        (
            "linux-arm64",
            "66237c656a3dd770db05cfd33c07c3710cbc74a3e00953105667fdeb91f36d8e",
            "44ad4e92dd4260a6f9689cf4d4839c4bc3e58adb11e6faeacb68c5740acee1a9",
        ),
    ];
    for (name, archive, binary) in expected {
        let target = lock.targets.get(name).unwrap();
        assert_eq!(target.archive_sha256, archive);
        assert_eq!(target.binary_sha256, binary);
    }
}

fn gzip_bytes(payload: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(payload).unwrap();
    encoder.finish().unwrap()
}

fn write_package_assets(dir: &Path, lock_json: &str) {
    fs::write(dir.join("workerd.lock.json"), lock_json).unwrap();
    fs::write(dir.join("config.capnp"), b"const config = ();\n").unwrap();
    fs::create_dir(dir.join("system-workers")).unwrap();
    fs::write(dir.join("system-workers/host.js"), b"export default {};\n").unwrap();
}

fn write_package_runbooks(dir: &Path) {
    fs::create_dir(dir).unwrap();
    for name in [
        "install-and-first-start.md",
        "backup-and-retention.md",
        "fresh-host-restore.md",
        "upgrade-and-rollback.md",
        "disk-pressure.md",
        "sqlite-corruption.md",
        "s3-outage.md",
        "workerd-crash-loop.md",
        "master-key-loss-and-recovery.md",
        "scheduler-recovery.md",
        "collect-support-bundle.md",
    ] {
        fs::write(dir.join(name), format!("# {name}\n")).unwrap();
    }
}

fn no_partial_bundles(parent: &Path) {
    let leftovers: Vec<_> = fs::read_dir(parent)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".partial-bundle"))
        .collect();
    assert!(leftovers.is_empty(), "staging leaked {leftovers:?}");
}

#[test]
fn package_release_is_atomic_and_rejects_bad_inputs() {
    let dir = TempDir::new().unwrap();
    let payload = version_script(None);
    let gz = gzip_bytes(payload.as_bytes());
    let mut lock_txt = lock_json(&sha256_bytes(payload.as_bytes()), "");
    lock_txt = lock_txt.replace(
        "22657ec7045a3677b7f52e97f106fe0493add57810687e755e8c6f4fba4b1dba",
        &sha256_bytes(&gz),
    );
    let lock = RuntimeLock::parse(lock_txt.as_bytes()).unwrap();
    let assets = dir.path().join("assets");
    fs::create_dir(&assets).unwrap();
    write_package_assets(&assets, &lock_txt);
    let platformd = dir.path().join("platformd");
    write_exec(&platformd, "#!/bin/sh\necho platformd\n");
    let license = dir.path().join("LICENSE");
    fs::write(&license, b"Apache-2.0\n").unwrap();
    let default_config = dir.path().join("default.toml");
    fs::write(
        &default_config,
        b"[server]\npublic_bind = \"127.0.0.1:1\"\n",
    )
    .unwrap();
    let runbooks = dir.path().join("runbooks");
    write_package_runbooks(&runbooks);

    let dest = dir.path().join("rel dest");
    package_release_bundle(&PackageReleaseRequest {
        lock: &lock,
        dest_dir: &dest,
        platformd: &platformd,
        assets_dir: &assets,
        license_file: &license,
        default_config: &default_config,
        runbooks_dir: &runbooks,
        release_json: b"{}",
        download: false,
        archive_bytes: Some(&gz),
    })
    .unwrap();
    assert!(dest.join("bin/workerd").is_file());
    assert!(dest.join("bin/platformd").is_file());
    assert!(dest.join("runtime/config.capnp").is_file());
    let mut top_level: Vec<_> = fs::read_dir(&dest)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    top_level.sort();
    assert_eq!(
        top_level,
        ["bin", "docs", "licenses", "runtime", "share"].map(std::ffi::OsString::from)
    );
    assert!(dest.join("share/release.json").is_file());
    assert!(dest.join("docs/runbooks/fresh-host-restore.md").is_file());
    assert!(!dest.join(".workerd-inst").exists());
    no_partial_bundles(dir.path());

    let marker = b"keep-me";
    let existing = dir.path().join("already");
    fs::write(&existing, marker).unwrap();
    let err = package_release_bundle(&PackageReleaseRequest {
        lock: &lock,
        dest_dir: &existing,
        platformd: &platformd,
        assets_dir: &assets,
        license_file: &license,
        default_config: &default_config,
        runbooks_dir: &runbooks,
        release_json: b"{}",
        download: false,
        archive_bytes: Some(&gz),
    })
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    assert_eq!(fs::read(&existing).unwrap(), marker);

    let bad_hash = package_release_bundle(&PackageReleaseRequest {
        lock: &lock,
        dest_dir: &dir.path().join("bad-hash"),
        platformd: &platformd,
        assets_dir: &assets,
        license_file: &license,
        default_config: &default_config,
        runbooks_dir: &runbooks,
        release_json: b"{}",
        download: false,
        archive_bytes: Some(b"not-the-archive"),
    });
    assert!(bad_hash.is_err());
    assert!(!dir.path().join("bad-hash").exists());
    no_partial_bundles(dir.path());

    let missing_capnp = dir.path().join("assets-missing");
    fs::create_dir(&missing_capnp).unwrap();
    fs::write(missing_capnp.join("workerd.lock.json"), &lock_txt).unwrap();
    let late = package_release_bundle(&PackageReleaseRequest {
        lock: &lock,
        dest_dir: &dir.path().join("late-asset"),
        platformd: &platformd,
        assets_dir: &missing_capnp,
        license_file: &license,
        default_config: &default_config,
        runbooks_dir: &runbooks,
        release_json: b"{}",
        download: false,
        archive_bytes: Some(&gz),
    });
    assert!(late.is_err());
    assert!(!dir.path().join("late-asset").exists());
    no_partial_bundles(dir.path());

    let link = dir.path().join("linked-platformd");
    symlink(&platformd, &link).unwrap();
    let sym = package_release_bundle(&PackageReleaseRequest {
        lock: &lock,
        dest_dir: &dir.path().join("from-symlink"),
        platformd: &link,
        assets_dir: &assets,
        license_file: &license,
        default_config: &default_config,
        runbooks_dir: &runbooks,
        release_json: b"{}",
        download: false,
        archive_bytes: Some(&gz),
    });
    assert!(sym.is_err());
    assert!(!dir.path().join("from-symlink").exists());
    no_partial_bundles(dir.path());
}

fn outside_mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}

struct AncestorFixture {
    _dir: TempDir,
    outside: PathBuf,
    linked_leaf: PathBuf,
    sentinel: PathBuf,
    sentinel_mode: u32,
    sentinel_bytes: Vec<u8>,
}

fn ancestor_fixture(relative: bool, leaf: &str) -> AncestorFixture {
    let dir = TempDir::new().unwrap();
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let sentinel = outside.join("sentinel");
    fs::write(&sentinel, b"outside-sentinel").unwrap();
    let mut perms = fs::metadata(&sentinel).unwrap().permissions();
    perms.set_mode(0o644);
    fs::set_permissions(&sentinel, perms).unwrap();
    let base = dir.path().join("base");
    fs::create_dir(&base).unwrap();
    let link = base.join("link");
    if relative {
        symlink(Path::new("../outside"), &link).unwrap();
    } else {
        symlink(&outside, &link).unwrap();
    }
    let sentinel_mode = outside_mode(&sentinel);
    AncestorFixture {
        linked_leaf: link.join(leaf),
        outside,
        sentinel,
        sentinel_mode,
        sentinel_bytes: b"outside-sentinel".to_vec(),
        _dir: dir,
    }
}

fn assert_outside_untouched(fx: &AncestorFixture) {
    assert_eq!(fs::read(&fx.sentinel).unwrap(), fx.sentinel_bytes);
    assert_eq!(outside_mode(&fx.sentinel), fx.sentinel_mode);
    let names: Vec<_> = fs::read_dir(&fx.outside)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect();
    assert!(
        names.iter().all(|n| n != "created-by-runtime"),
        "runtime must not create files in the outside directory: {names:?}"
    );
}

#[test]
fn write_atomic_new_destination_appears_race_preserves_winner() {
    let dir = TempDir::new().unwrap();
    let dest = dir.path().join("atom");
    set_publish_hook({
        let dest = dest.clone();
        move |path| {
            if path == dest {
                fs::write(&dest, b"winner-bytes").unwrap();
                let mut p = fs::metadata(&dest).unwrap().permissions();
                p.set_mode(0o600);
                fs::set_permissions(&dest, p).unwrap();
            }
        }
    });
    let err = write_atomic_new(&dest, b"loser-bytes", FILE_MODE).unwrap_err();
    clear_publish_hook();
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    assert_eq!(fs::read(&dest).unwrap(), b"winner-bytes");
    let leftovers: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect();
    assert!(
        leftovers
            .iter()
            .all(|n| !n.to_string_lossy().contains("partial")),
        "temp file must be removed: {leftovers:?}"
    );
}

#[test]
fn install_release_destination_appears_race_preserves_bytes() {
    let dir = TempDir::new().unwrap();
    let payload = version_script(None);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(payload.as_bytes()).unwrap();
    let gz = encoder.finish().unwrap();
    let json = lock_json(&sha256_bytes(payload.as_bytes()), "").replace(
        "22657ec7045a3677b7f52e97f106fe0493add57810687e755e8c6f4fba4b1dba",
        &sha256_bytes(&gz),
    );
    let lock = RuntimeLock::parse(json.as_bytes()).unwrap();
    let dest = dir.path().join("rel");
    set_publish_hook({
        let dest = dest.clone();
        move |path| {
            if path == dest {
                fs::create_dir(&dest).unwrap();
                fs::write(dest.join("keep-me"), b"concurrent-winner").unwrap();
            }
        }
    });
    let err = install_official_release(&lock, &dest, false, Some(&gz)).unwrap_err();
    clear_publish_hook();
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    assert_eq!(
        fs::read(dest.join("keep-me")).unwrap(),
        b"concurrent-winner"
    );
}

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
            runtime.lock(),
            runtime.lock_bytes(),
            &runtime,
            &platform,
            &token,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_digest_cache_lookup_cannot_delete_publish_window() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let counter = dir.path().join("count");
    let args = dir.path().join("args");
    let bin = dir.path().join("workerd");
    write_exec(&bin, &compile_script(&counter, &args, "COMPILED-BYTES", ""));
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let platform = platform_meta();
    let dest_name = {
        let (digest, _, _) = digest_for(
            dir.path(),
            runtime.lock(),
            runtime.lock_bytes(),
            &runtime,
            &platform,
            &token,
        )
        .unwrap();
        format!("config.{digest}.bin")
    };
    let dest = data.join(&dest_name);
    let sidecar = dest.with_extension("bin.digest");
    let paused = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let c_started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    set_after_config_rename_hook({
        let dest = dest.clone();
        let paused = paused.clone();
        let release = release.clone();
        move |path| {
            if path == dest {
                paused.store(true, std::sync::atomic::Ordering::SeqCst);
                let wait_from = std::time::Instant::now();
                while !release.load(std::sync::atomic::Ordering::SeqCst)
                    && wait_from.elapsed() < Duration::from_secs(5)
                {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
    });
    let a = tokio::spawn({
        let runtime = runtime.clone();
        let lock_path = lock_path.clone();
        let assets = dir.path().to_path_buf();
        let data = data.clone();
        let platform = platform.clone();
        let token = SecretString::new(TOKEN);
        async move {
            let redactor = redactor_with_token();
            compile_static_config(compile_req(
                &runtime,
                &lock_path,
                &assets,
                &data,
                &platform,
                &token,
                &redactor,
                Duration::from_secs(8),
            ))
            .await
        }
    });
    let wait_from = std::time::Instant::now();
    while !paused.load(std::sync::atomic::Ordering::SeqCst)
        && wait_from.elapsed() < Duration::from_secs(5)
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        paused.load(std::sync::atomic::Ordering::SeqCst),
        "publisher A must pause after config rename"
    );
    assert!(dest.exists(), "publisher A must have renamed the config");
    assert!(
        !sidecar.exists(),
        "sidecar must not exist while A is paused"
    );
    let c = tokio::spawn({
        let runtime = runtime.clone();
        let lock_path = lock_path.clone();
        let assets = dir.path().to_path_buf();
        let data = data.clone();
        let platform = platform.clone();
        let token = SecretString::new(TOKEN);
        let c_started = c_started.clone();
        async move {
            c_started.store(true, std::sync::atomic::Ordering::SeqCst);
            let redactor = redactor_with_token();
            compile_static_config(compile_req(
                &runtime,
                &lock_path,
                &assets,
                &data,
                &platform,
                &token,
                &redactor,
                Duration::from_secs(8),
            ))
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        c_started.load(std::sync::atomic::Ordering::SeqCst),
        "caller C must have started"
    );
    assert!(
        !c.is_finished(),
        "caller C must block on the digest gate instead of deleting A's transient config"
    );
    assert!(dest.exists());
    assert!(!sidecar.exists());
    assert!(!a.is_finished(), "publisher A must still hold the gate");
    release.store(true, std::sync::atomic::Ordering::SeqCst);
    let (ra, rc) = tokio::join!(a, c);
    clear_after_config_rename_hook();
    let ca = ra.expect("join A").expect("publisher A must succeed");
    let cc = rc.expect("join C").expect("caller C must reuse the winner");
    assert_eq!(ca.digest(), cc.digest());
    ca.open().expect("winner must revalidate");
    cc.open().expect("caller must revalidate the same winner");
}

#[tokio::test]
async fn cancel_sends_term_before_process_is_gone() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let pid_file = dir.path().join("pid");
    let marker = dir.path().join("term-marker");
    let bin = dir.path().join("workerd");
    write_exec(
        &bin,
        &format!(
            "#!/bin/sh
if [ \"$1\" = \"--version\" ]; then echo '{VERSION}'; exit 0; fi
trap 'echo term > \"{marker}\"; exit 0' TERM
echo $$ > '{pid}'
sleep 30
",
            pid = pid_file.display(),
            marker = marker.display(),
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
    let mut fut = Box::pin(compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(30),
    )));
    let pid = loop {
        tokio::select! {
            _ = fut.as_mut() => panic!("compile finished before cancellation"),
            _ = tokio::time::sleep(Duration::from_millis(20)) => {
                if let Ok(contents) = fs::read_to_string(&pid_file)
                    && let Ok(pid) = contents.trim().parse::<i32>()
                {
                    break pid;
                }
            }
        }
    };
    drop(fut);
    let started = std::time::Instant::now();
    loop {
        if marker.exists() {
            break;
        }
        if !pid_alive(pid) {
            panic!("process exited before the TERM marker was written");
        }
        if started.elapsed() > Duration::from_secs(2) {
            panic!("TERM marker was not written");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    wait_pid_gone(pid, Duration::from_secs(4)).expect("pid gone after TERM");
    wait_reaped(pid, Duration::from_secs(4)).expect("reaped after TERM");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fallback_does_not_signal_after_owner_reaps() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let pid_file = dir.path().join("pid");
    let bin = dir.path().join("workerd");
    write_exec(
        &bin,
        &format!(
            "#!/bin/sh
if [ \"$1\" = \"--version\" ]; then echo '{VERSION}'; exit 0; fi
echo $$ > '{pid}'
sleep 30
",
            pid = pid_file.display(),
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
    let owner_reaped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    set_owner_reaped_hook({
        let owner_reaped = owner_reaped.clone();
        move || owner_reaped.store(true, std::sync::atomic::Ordering::SeqCst)
    });
    let mut fut = Box::pin(compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(30),
    )));
    let pid = loop {
        tokio::select! {
            _ = fut.as_mut() => panic!("compile finished before cancellation"),
            _ = tokio::time::sleep(Duration::from_millis(20)) => {
                if let Ok(contents) = fs::read_to_string(&pid_file)
                    && let Ok(pid) = contents.trim().parse::<i32>()
                {
                    break pid;
                }
            }
        }
    };
    drop(fut);
    let started = std::time::Instant::now();
    while !owner_reaped.load(std::sync::atomic::Ordering::SeqCst)
        && started.elapsed() < Duration::from_secs(4)
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        owner_reaped.load(std::sync::atomic::Ordering::SeqCst),
        "owner must reap before the delayed fallback runs"
    );
    wait_reaped(pid, Duration::from_secs(4)).expect("owner reaped the original child");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "owner must finish TERM/KILL/reap without a delayed fallback thread"
    );
    clear_owner_reaped_hook();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_term_ignored_still_kills_descendants() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let pid_file = dir.path().join("pid");
    let child_file = dir.path().join("child");
    let bin = dir.path().join("workerd");
    write_exec(
        &bin,
        &format!(
            "#!/bin/sh
if [ \"$1\" = \"--version\" ]; then echo '{VERSION}'; exit 0; fi
echo $$ > '{pid}'
trap '' TERM
sleep 30 &
echo $! > '{child}'
while true; do sleep 0.05; done
",
            pid = pid_file.display(),
            child = child_file.display(),
        ),
    );
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let redactor = redactor_with_token();
    let platform = platform_meta();
    let mut fut = Box::pin(compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(30),
    )));
    let wait_pid = async {
        let started = std::time::Instant::now();
        while !pid_file.exists() && started.elapsed() < Duration::from_secs(2) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let pid: i32 = fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        while !child_file.exists() && started.elapsed() < Duration::from_secs(2) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let child: i32 = fs::read_to_string(&child_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        (pid, child)
    };
    let (pid, child) = tokio::select! {
        _ = fut.as_mut() => panic!("compile finished before cancellation"),
        ids = wait_pid => ids,
    };
    let started = std::time::Instant::now();
    drop(fut);
    wait_reaped(pid, Duration::from_secs(4)).expect("parent killed");
    wait_pid_gone(child, Duration::from_secs(4)).expect("descendant killed");
    assert!(started.elapsed() < Duration::from_secs(3));
    let leftovers: Vec<_> = fs::read_dir(&data)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect();
    assert!(
        leftovers.is_empty(),
        "work directories must be removed: {leftovers:?}"
    );
}

#[tokio::test]
async fn symlink_ancestor_rejected_for_all_external_paths() {
    for relative in [false, true] {
        let fx = ancestor_fixture(relative, "workerd");
        write_exec(&fx.outside.join("workerd"), &version_script(None));
        let dummy = TempDir::new().unwrap();
        let bin_ok = dummy.path().join("workerd");
        write_exec(&bin_ok, &version_script(None));
        let lock_ok = write_lock(dummy.path(), &sha256_file(&bin_ok));
        let err = verify_runtime_binary(
            &lock_ok,
            &fx.linked_leaf,
            Duration::from_secs(2),
            &Redactor::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::PathInvalid);
        assert_outside_untouched(&fx);

        let fx = ancestor_fixture(relative, "workerd.lock.json");
        fs::write(
            fx.outside.join("workerd.lock.json"),
            lock_json(&"ab".repeat(32), ""),
        )
        .unwrap();
        let err = load_runtime_lock(&fx.linked_leaf).unwrap_err();
        assert_eq!(err.code(), ErrorCode::PathInvalid);
        assert_outside_untouched(&fx);

        let fx = ancestor_fixture(relative, "assets");
        fs::create_dir(fx.outside.join("assets")).unwrap();
        copy_formal_assets(&fx.outside.join("assets"));
        let dir = TempDir::new().unwrap();
        copy_formal_assets(dir.path());
        let bin = dir.path().join("workerd");
        write_exec(&bin, &version_script(None));
        let lock_path = write_lock(dir.path(), &sha256_file(&bin));
        let runtime = verify_ok(&lock_path, &bin).await;
        let data = dir.path().join("data");
        fs::create_dir(&data).unwrap();
        let token = SecretString::new(TOKEN);
        let redactor = redactor_with_token();
        let platform = platform_meta();
        let err = compile_static_config(compile_req(
            &runtime,
            &lock_path,
            &fx.linked_leaf,
            &data,
            &platform,
            &token,
            &redactor,
            Duration::from_secs(5),
        ))
        .await
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::PathInvalid);
        assert_outside_untouched(&fx);

        let fx = ancestor_fixture(relative, "data");
        let err = compile_static_config(compile_req(
            &runtime,
            &lock_path,
            dir.path(),
            &fx.linked_leaf,
            &platform,
            &token,
            &redactor,
            Duration::from_secs(5),
        ))
        .await
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::PathInvalid);
        assert_outside_untouched(&fx);

        let fx = ancestor_fixture(relative, "rel");
        let payload = version_script(None);
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(payload.as_bytes()).unwrap();
        let gz = encoder.finish().unwrap();
        let json = lock_json(&sha256_bytes(payload.as_bytes()), "").replace(
            "22657ec7045a3677b7f52e97f106fe0493add57810687e755e8c6f4fba4b1dba",
            &sha256_bytes(&gz),
        );
        let lock = RuntimeLock::parse(json.as_bytes()).unwrap();
        let err = install_official_release(&lock, &fx.linked_leaf, false, Some(&gz)).unwrap_err();
        assert_eq!(err.code(), ErrorCode::PathInvalid);
        assert_outside_untouched(&fx);
    }
}

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn descendant_holding_pipes_returns_within_deadline() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let pid_file = dir.path().join("pid");
    let child_file = dir.path().join("child");
    let bin = dir.path().join("workerd");
    write_exec(
        &bin,
        &format!(
            "#!/bin/sh
if [ \"$1\" = \"--version\" ]; then echo '{VERSION}'; exit 0; fi
echo $$ > '{pid}'
sleep 30 &
echo $! > '{child}'
exit 0
",
            pid = pid_file.display(),
            child = child_file.display(),
        ),
    );
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let redactor = redactor_with_token();
    let platform = platform_meta();
    let deadline = Duration::from_secs(2);
    let started = std::time::Instant::now();
    let result = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        deadline,
    ))
    .await;
    assert!(started.elapsed() < deadline + Duration::from_millis(800));
    if pid_file.exists() {
        let pid: i32 = fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        wait_reaped(pid, Duration::from_secs(2)).expect("pgid gone");
    }
    if child_file.exists() {
        let child: i32 = fs::read_to_string(&child_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        wait_pid_gone(child, Duration::from_secs(2)).expect("descendant gone");
    }
    if result.is_err() {
        let leftovers = leftover_names(&data);
        assert!(
            leftovers.iter().all(|n| {
                let s = n.to_string_lossy();
                !s.contains("partial") && !s.contains("compile")
            }),
            "partials must be removed: {leftovers:?}"
        );
    }
}

#[tokio::test]
async fn compile_stdout_write_failure_is_typed_and_cleans_up() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let counter = dir.path().join("count");
    let args = dir.path().join("args");
    let bin = dir.path().join("workerd");
    write_exec(&bin, &compile_script(&counter, &args, "COMPILED-BYTES", ""));
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let redactor = redactor_with_token();
    let platform = platform_meta();
    set_stdout_write_fail(true);
    let err = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    ))
    .await
    .unwrap_err();
    clear_io_fail_hooks();
    assert_eq!(err.code(), ErrorCode::ConfigCompileFailed);
    let leftovers = leftover_names(&data);
    assert!(
        leftovers
            .iter()
            .all(|n| !n.to_string_lossy().contains("partial")),
        "partials must be removed: {leftovers:?}"
    );
}

#[tokio::test]
async fn compile_stdout_read_failure_is_typed_and_cleans_up() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let counter = dir.path().join("count");
    let args = dir.path().join("args");
    let bin = dir.path().join("workerd");
    write_exec(&bin, &compile_script(&counter, &args, "COMPILED-BYTES", ""));
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let redactor = redactor_with_token();
    let platform = platform_meta();
    set_stdout_read_fail(true);
    let err = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    ))
    .await
    .unwrap_err();
    clear_io_fail_hooks();
    assert_eq!(err.code(), ErrorCode::RuntimeInvalid);
    let leftovers = leftover_names(&data);
    assert!(
        leftovers
            .iter()
            .all(|n| !n.to_string_lossy().contains("partial")),
        "partials must be removed: {leftovers:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_thread_spawn_failure_kills_spawned_child() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let pid_file = dir.path().join("pid");
    let child_file = dir.path().join("child");
    let bin = dir.path().join("workerd");
    write_exec(
        &bin,
        &format!(
            "#!/bin/sh
if [ \"$1\" = \"--version\" ]; then echo '{VERSION}'; exit 0; fi
echo $$ > '{pid}'
sleep 30 &
echo $! > '{child}'
sleep 30
",
            pid = pid_file.display(),
            child = child_file.display(),
        ),
    );
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let redactor = redactor_with_token();
    let platform = platform_meta();
    set_owner_spawn_fail_hook({
        let pid_file = pid_file.clone();
        let child_file = child_file.clone();
        move || {
            let started = std::time::Instant::now();
            while started.elapsed() < Duration::from_secs(2) {
                if pid_file.exists() && child_file.exists() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            true
        }
    });
    let err = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    ))
    .await
    .unwrap_err();
    clear_owner_spawn_fail_hook();
    assert_eq!(err.code(), ErrorCode::RuntimeInvalid);
    let pid: i32 = fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    wait_reaped(pid, Duration::from_secs(2)).expect("spawn-fail RAII reaped group");
    if child_file.exists() {
        let child: i32 = fs::read_to_string(&child_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        wait_pid_gone(child, Duration::from_secs(2)).expect("descendant reaped");
    }
    let leftovers = leftover_names(&data);
    assert!(
        leftovers
            .iter()
            .all(|n| !n.to_string_lossy().contains("partial")),
        "partials must be removed: {leftovers:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pgid_verify_failure_kills_spawned_child() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let pid_file = dir.path().join("pid");
    let child_file = dir.path().join("child");
    let bin = dir.path().join("workerd");
    write_exec(
        &bin,
        &format!(
            "#!/bin/sh
if [ \"$1\" = \"--version\" ]; then echo '{VERSION}'; exit 0; fi
echo $$ > '{pid}'
sleep 30 &
echo $! > '{child}'
sleep 30
",
            pid = pid_file.display(),
            child = child_file.display(),
        ),
    );
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let redactor = redactor_with_token();
    let platform = platform_meta();
    set_pgid_verify_fail_hook({
        let pid_file = pid_file.clone();
        let child_file = child_file.clone();
        move || {
            let started = std::time::Instant::now();
            while started.elapsed() < Duration::from_secs(2) {
                if pid_file.exists() && child_file.exists() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            true
        }
    });
    let err = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    ))
    .await
    .unwrap_err();
    clear_pgid_verify_fail_hook();
    assert_eq!(err.code(), ErrorCode::RuntimeInvalid);
    let pid: i32 = fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    wait_reaped(pid, Duration::from_secs(5)).expect("pgid-verify RAII reaped group");
    if child_file.exists() {
        let child: i32 = fs::read_to_string(&child_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        wait_pid_gone(child, Duration::from_secs(5)).expect("descendant reaped");
    }
    let leftovers = leftover_names(&data);
    assert!(
        leftovers
            .iter()
            .all(|n| !n.to_string_lossy().contains("partial")),
        "partials must be removed: {leftovers:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_failure_drop_still_reaps_group() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let pid_file = dir.path().join("pid");
    let child_file = dir.path().join("child");
    let bin = dir.path().join("workerd");
    write_exec(
        &bin,
        &format!(
            "#!/bin/sh
if [ \"$1\" = \"--version\" ]; then echo '{VERSION}'; exit 0; fi
echo $$ > '{pid}'
sleep 30 &
echo $! > '{child}'
sleep 30
",
            pid = pid_file.display(),
            child = child_file.display(),
        ),
    );
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let redactor = redactor_with_token();
    let platform = platform_meta();
    set_wait_fail_hook({
        let pid_file = pid_file.clone();
        let child_file = child_file.clone();
        move || pid_file.exists() && child_file.exists()
    });
    let deadline = Duration::from_secs(1);
    let started = std::time::Instant::now();
    let err = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        deadline,
    ))
    .await
    .unwrap_err();
    let elapsed = started.elapsed();
    clear_io_fail_hooks();
    assert_eq!(err.code(), ErrorCode::RuntimeInvalid);
    assert!(
        elapsed < deadline + Duration::from_millis(200) + Duration::from_secs(2),
        "wait-fail cleanup must not wait for the 30s fixture: {elapsed:?}"
    );
    let pid: i32 = fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    wait_reaped(pid, Duration::from_secs(2)).expect("wait-fail Drop reaped group");
    if child_file.exists() {
        let child: i32 = fs::read_to_string(&child_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        wait_pid_gone(child, Duration::from_secs(2)).expect("descendant reaped");
    }
    let leftovers = leftover_names(&data);
    assert!(
        leftovers
            .iter()
            .all(|n| !n.to_string_lossy().contains("partial")),
        "partials must be removed: {leftovers:?}"
    );
}

#[tokio::test]
async fn reader_panic_is_typed_and_cleans_up() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let counter = dir.path().join("count");
    let args = dir.path().join("args");
    let bin = dir.path().join("workerd");
    write_exec(&bin, &compile_script(&counter, &args, "COMPILED-BYTES", ""));
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let redactor = redactor_with_token();
    let platform = platform_meta();
    set_reader_panic(true);
    let deadline = Duration::from_secs(5);
    let started = std::time::Instant::now();
    let err = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        deadline,
    ))
    .await
    .unwrap_err();
    let elapsed = started.elapsed();
    clear_io_fail_hooks();
    assert_eq!(err.code(), ErrorCode::RuntimeInvalid);
    assert!(
        elapsed < Duration::from_secs(2),
        "reader panic must not wait out the command deadline: {elapsed:?}"
    );
    let leftovers = leftover_names(&data);
    assert!(
        leftovers
            .iter()
            .all(|n| !n.to_string_lossy().contains("partial")),
        "partials must be removed: {leftovers:?}"
    );
}
