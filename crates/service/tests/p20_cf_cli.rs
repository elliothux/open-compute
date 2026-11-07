//! Real CLI/process coverage for the P12 target registry and cf replacement boundary.

use rustix::process::{Pid, Signal, kill_process};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

const ACCOUNT_ID: &str = "0123456789ab7def8123456789abcdef";

fn ocd(config_home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ocd"));
    command
        .env("OPEN_COMPUTE_TEST_OCD_ROOT", config_home)
        .arg("--no-update-check");
    command
}

fn run(command: &mut Command) -> Output {
    command.output().unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn serve_target(requests: usize) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        for _ in 0..requests {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let count = stream.read(&mut chunk).unwrap();
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..count]);
                if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(bytes).unwrap();
            assert!(request.contains("authorization: Bearer fixture-deployer-token\r\n"));
            let first_line = request.lines().next().unwrap();
            let body = if first_line.contains("/accounts/") {
                format!(r#"{{"success":true,"result":{{"id":"{ACCOUNT_ID}"}}}}"#)
            } else {
                r#"{"success":true,"result":{"cf_version":"1.0.0-beta.21"}}"#.to_owned()
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        }
    });
    (format!("http://{address}/client/v4"), handle)
}

fn write_fake_cf(project: &Path, api_base_url: &str, started: &Path, version: &str) -> PathBuf {
    // The package declares its JavaScript entry; `.bin/cf` is a shell shim node must not run.
    let package = project.join("node_modules/cf");
    let bin = package.join("bin/cf");
    fs::create_dir_all(bin.parent().unwrap()).unwrap();
    fs::write(
        package.join("package.json"),
        r#"{"name":"cf","bin":{"cf":"./bin/cf"}}"#,
    )
    .unwrap();
    let shim = project.join("node_modules/.bin/cf");
    fs::create_dir_all(shim.parent().unwrap()).unwrap();
    fs::write(
        &shim,
        "#!/bin/sh\nexec node \"$(dirname \"$0\")/../cf/bin/cf\" \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
    let script = format!(
        r#"const args = process.argv.slice(2);
if (args[0] === '--version') {{ console.log('{version}'); process.exit(0); }}
const required = {{
  CLOUDFLARE_API_BASE_URL: '{api_base_url}', CLOUDFLARE_API_TOKEN: 'fixture-deployer-token',
  CLOUDFLARE_ACCOUNT_ID: '{ACCOUNT_ID}', WRANGLER_LOG_SANITIZE: 'true',
  CF_SEND_TELEMETRY: 'false', DO_NOT_TRACK: '1'
}};
for (const [key, value] of Object.entries(required)) {{
  if (process.env[key] !== value) process.exit(81);
}}
for (const key of ['CLOUDFLARE_API_KEY', 'CLOUDFLARE_EMAIL', 'CF_API_TOKEN', 'CF_API_BASE_URL', 'CF_ACCOUNT_ID']) {{
  if (key in process.env) process.exit(82);
}}
if (args[0] === 'hold') {{
  const fs = require('node:fs'); fs.writeFileSync('{}', '');
  setInterval(() => {{}}, 1000);
}} else {{
  if (JSON.stringify(args) !== JSON.stringify(['deploy', '--config', '配置.jsonc', '', '--cwd', 'nested'])) process.exit(92);
  process.exit(37);
}}
"#,
        started.display()
    );
    fs::write(&bin, script).unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

#[test]
fn target_commands_and_cf_wrapper_preserve_the_day1_boundary() {
    let temp = TempDir::new().unwrap();
    let config_home = temp.path().join("config-home");
    fs::create_dir(&config_home).unwrap();
    fs::set_permissions(&config_home, fs::Permissions::from_mode(0o700)).unwrap();
    let user_root = config_home.join("user");
    fs::create_dir(&user_root).unwrap();
    fs::set_permissions(user_root, fs::Permissions::from_mode(0o700)).unwrap();
    let token = temp.path().join("deployer.token");
    fs::write(&token, "fixture-deployer-token\n").unwrap();
    fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
    let (api_base_url, server) = serve_target(5);

    let add = run(ocd(&config_home)
        .args(["target", "add", "remote", "--api-base-url"])
        .arg(&api_base_url)
        .args(["--instance-id", ACCOUNT_ID, "--token-file"])
        .arg(&token));
    assert_success(&add);
    assert!(!String::from_utf8_lossy(&add.stdout).contains("fixture-deployer-token"));

    let list = run(ocd(&config_home).args(["target", "list", "--json"]));
    assert_success(&list);
    let list_body = String::from_utf8(list.stdout).unwrap();
    assert!(list_body.contains("\"name\":\"remote\""));
    assert!(!list_body.contains("fixture-deployer-token"));

    let show = run(ocd(&config_home).args(["target", "show", "remote", "--json"]));
    assert_success(&show);
    assert!(!String::from_utf8_lossy(&show.stdout).contains("fixture-deployer-token"));

    let test = run(ocd(&config_home).args(["target", "test", "remote", "--json"]));
    assert_success(&test);
    let test_body = String::from_utf8(test.stdout).unwrap();
    assert!(test_body.contains("\"cf_version\":\"1.0.0-beta.21\""));
    assert!(!test_body.contains("fixture-deployer-token"));

    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("cloudflare.config.ts"), "export default {};\n").unwrap();
    let started = temp.path().join("cf-started");
    write_fake_cf(&project, &api_base_url, &started, "1.0.0-beta.21");
    let wrapped = run(ocd(&config_home)
        .env("CLOUDFLARE_API_KEY", "legacy-key")
        .env("CLOUDFLARE_EMAIL", "legacy@example.invalid")
        .env("CF_API_TOKEN", "legacy-token")
        .env("CF_API_BASE_URL", "https://wrong.invalid/client/v4")
        .env("CF_ACCOUNT_ID", "wrong-account")
        .args(["cf", "--target", "remote", "--project"])
        .arg(&project)
        .args(["deploy", "--config", "配置.jsonc", "", "--cwd", "nested"]));
    assert_eq!(wrapped.status.code(), Some(37));
    let wrapped_stderr = String::from_utf8(wrapped.stderr).unwrap();
    assert!(wrapped_stderr.contains("CF_TARGET kind=target name=remote"));
    assert!(!wrapped_stderr.contains("fixture-deployer-token"));

    write_fake_cf(&project, &api_base_url, &started, "1.1.0");
    let cross_major = run(ocd(&config_home)
        .args(["cf", "--target", "remote", "--project"])
        .arg(&project)
        .args(["deploy", "--config", "配置.jsonc", "", "--cwd", "nested"]));
    assert_eq!(cross_major.status.code(), Some(37));
    let cross_major_stderr = String::from_utf8(cross_major.stderr).unwrap();
    assert!(cross_major_stderr.contains("CF_VERSION_WARNING"));
    assert!(cross_major_stderr.contains("detected=1.1.0 certified=1.0.0-beta.21"));
    assert!(cross_major_stderr.contains("certified_cf=1.0.0-beta.21"));
    assert!(!cross_major_stderr.contains("fixture-deployer-token"));

    let terminated = ocd(&config_home)
        .args(["cf", "--target", "remote", "--project"])
        .arg(&project)
        .arg("hold")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    for _ in 0..100 {
        if started.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(started.exists(), "replacement cf did not start");
    kill_process(
        Pid::from_raw(terminated.id().try_into().unwrap()).unwrap(),
        Signal::TERM,
    )
    .unwrap();
    let terminated = terminated.wait_with_output().unwrap();
    assert_eq!(terminated.status.signal(), Some(Signal::TERM.as_raw()));
    assert!(!String::from_utf8_lossy(&terminated.stderr).contains("fixture-deployer-token"));

    let remove = run(ocd(&config_home).args(["target", "remove", "remote"]));
    assert_success(&remove);
    assert!(token.exists());
    let empty = run(ocd(&config_home).args(["target", "list", "--json"]));
    assert_success(&empty);
    assert!(
        String::from_utf8(empty.stdout)
            .unwrap()
            .contains("\"targets\":[]")
    );

    server.join().unwrap();
}
