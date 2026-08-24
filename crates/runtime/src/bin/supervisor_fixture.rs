//! Test-only workerd stand-in implementing control-fd 3, HTTP probe, and faults.

#![allow(missing_docs)]

use serde::Deserialize;
use std::fs;
#[cfg(not(target_os = "linux"))]
use std::fs::File;
use std::io::{Read, Write};
use std::net::TcpListener;
#[cfg(not(target_os = "linux"))]
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

const READY_PATH: &str = "/internal/ready";
const TOKEN_HEADER: &str = "x-open-compute-internal-token";
const VERSION: &str = "workerd 2026-08-23";

#[derive(Debug, Deserialize, Default)]
struct FixtureConfig {
    #[serde(default = "default_mode")]
    mode: String,
    #[serde(default)]
    token: String,
    #[serde(default)]
    argv_path: Option<PathBuf>,
    #[serde(default)]
    child_pid_path: Option<PathBuf>,
    #[serde(default)]
    stdin_marker_path: Option<PathBuf>,
}

fn default_mode() -> String {
    "ready".into()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--version") {
        println!("{VERSION}");
        return;
    }
    if args.first().map(String::as_str) == Some("--hold-pipes") {
        hold_pipes_ignore_term();
        return;
    }
    let mut stdin = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut stdin);
    let cfg: FixtureConfig = serde_json::from_slice(&stdin).unwrap_or_default();
    if let Some(path) = &cfg.argv_path {
        let _ = fs::write(path, serde_json::to_vec(&args).expect("argv json"));
    }
    if let Some(path) = &cfg.stdin_marker_path {
        let _ = fs::write(path, &stdin);
    }

    match cfg.mode.as_str() {
        "early_exit" => std::process::exit(7),
        "no_control" => loop {
            std::thread::sleep(Duration::from_secs(30));
        },
        "malformed_control" => {
            write_control(b"not-json\n");
            hang();
        }
        "oversized_control" => {
            let mut line = vec![b'x'; 5000];
            line.push(b'\n');
            write_control(&line);
            hang();
        }
        "duplicate_control" => {
            write_listen(43100);
            write_listen(43100);
            hang();
        }
        "wrong_socket" => {
            write_control(br#"{"event":"listen","socket":"https","port":43100}"#);
            write_control(b"\n");
            hang();
        }
        "non_loopback" => {
            write_control(
                br#"{"event":"listen","socket":"http","port":43100,"address":"8.8.8.8:43100"}"#,
            );
            write_control(b"\n");
            hang();
        }
        "timeout" => hang(),
        "secret_logs" => {
            eprintln!("token={}", cfg.token);
            eprintln!("Authorization: Bearer {}", cfg.token);
            eprintln!("x-open-compute-internal-token: {}", cfg.token);
            eprintln!("path=/secret/token-path");
            let _ = std::io::stderr().write_all(&[0xff, 0xfe, b'\n']);
            let oversized = "A".repeat(16 * 1024);
            eprintln!("{oversized}");
            println!("stdout secret {}", cfg.token);
            let _ = std::io::stdout().write_all(&[0x80, 0x81, b'\n']);
            serve_ready_then_exit(&cfg, 42);
        }
        "non_utf8" => {
            let _ = std::io::stderr().write_all(&[0xff, 0xfe, b'\n']);
            let _ = std::io::stdout().write_all(&[0x80, 0x81, b'\n']);
            serve_ready(&cfg, false);
        }
        "oversize_logs" => {
            let line = "A".repeat(16 * 1024);
            eprintln!("{line}");
            println!("{line}");
            serve_ready(&cfg, false);
        }
        "bind_fail" => {
            write_listen(1);
            hang();
        }
        "ignore_term" => {
            let flag = Arc::new(AtomicBool::new(false));
            let _ = signal_hook::flag::register(signal_hook::consts::SIGTERM, flag);
            serve_ready(&cfg, true);
        }
        "crash_after_ready" => {
            serve_ready(&cfg, false);
        }
        "late_duplicate_control" => serve_ready_then_control(&cfg, true),
        "late_malformed_control" => serve_ready_then_control(&cfg, false),
        "slow_probe" => {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            let port = listener.local_addr().expect("addr").port();
            write_listen(port);
            hang();
        }
        "child" => {
            spawn_descendant(cfg.child_pid_path.as_deref());
            serve_ready(&cfg, true);
        }
        "child_ignore_term" => {
            spawn_hold_pipes_descendant(cfg.child_pid_path.as_deref());
            serve_ready(&cfg, true);
        }
        _ => serve_ready(&cfg, true),
    }
}

fn hang() {
    loop {
        std::thread::sleep(Duration::from_secs(30));
    }
}

#[cfg(not(target_os = "linux"))]
fn write_control(bytes: &[u8]) {
    if let Ok(mut sock) = File::open("/dev/fd/3") {
        let _ = sock.write_all(bytes);
        let _ = sock.flush();
        return;
    }
    if let Ok(mut sock) = UnixStream::connect("/dev/fd/3") {
        let _ = sock.write_all(bytes);
    }
}

#[cfg(target_os = "linux")]
fn write_control(bytes: &[u8]) {
    // Linux cannot reopen a Unix socket through /dev/fd. Let a short-lived
    // child inherit fd 3 and bridge stdin to it without claiming raw-fd
    // ownership in this unsafe-free test fixture.
    let Ok(mut child) = Command::new("/bin/sh")
        .args(["-c", "/bin/cat >&3"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(bytes);
    }
    let _ = child.wait();
}

fn write_listen(port: u16) {
    let msg = format!("{{\"event\":\"listen\",\"socket\":\"http\",\"port\":{port}}}\n");
    write_control(msg.as_bytes());
}

fn hold_pipes_ignore_term() {
    let flag = Arc::new(AtomicBool::new(false));
    let _ = signal_hook::flag::register(signal_hook::consts::SIGTERM, flag);
    loop {
        let _ = std::io::stdout().write_all(b"hold\n");
        let _ = std::io::stderr().write_all(b"hold\n");
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn spawn_hold_pipes_descendant(path: Option<&std::path::Path>) {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("/bin/sleep"));
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--hold-pipes");
    cmd.stdin(std::process::Stdio::null());
    if let Ok(child) = cmd.spawn() {
        if let Some(path) = path {
            let _ = fs::write(path, child.id().to_string());
        }
        std::mem::forget(child);
    }
}

fn spawn_descendant(path: Option<&std::path::Path>) {
    let mut cmd = std::process::Command::new("/bin/sleep");
    cmd.arg("60");
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    if let Ok(child) = cmd.spawn() {
        if let Some(path) = path {
            let _ = fs::write(path, child.id().to_string());
        }
        std::mem::forget(child);
    }
}

fn serve_ready_then_control(cfg: &FixtureConfig, duplicate: bool) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    write_listen(port);
    listener.set_nonblocking(false).expect("blocking");
    let token = cfg.token.clone();
    let mut extra_sent = false;
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let ok = req.starts_with("GET /internal/ready ")
                    && req.contains(&format!("{TOKEN_HEADER}: {token}"))
                    && req.contains("\r\n\r\n");
                let resp = if ok {
                    "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n"
                } else {
                    "HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n"
                };
                let _ = stream.write_all(resp.as_bytes());
                if ok && !extra_sent {
                    extra_sent = true;
                    if duplicate {
                        write_listen(port);
                    } else {
                        write_control(b"not-json\n");
                    }
                }
            }
            Err(_) => hang(),
        }
    }
}

fn serve_ready_then_exit(cfg: &FixtureConfig, code: i32) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    write_listen(port);
    listener.set_nonblocking(false).expect("blocking");
    let token = cfg.token.clone();
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let ok = req.starts_with("GET /internal/ready ")
                    && req.contains(&format!("{TOKEN_HEADER}: {token}"))
                    && req.contains("\r\n\r\n");
                let resp = if ok {
                    "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n"
                } else {
                    "HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n"
                };
                let _ = stream.write_all(resp.as_bytes());
                if ok {
                    std::process::exit(code);
                }
            }
            Err(_) => hang(),
        }
    }
}

fn serve_ready(cfg: &FixtureConfig, persist: bool) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    write_listen(port);
    listener.set_nonblocking(false).expect("blocking");
    let token = cfg.token.clone();
    let crash = cfg.mode == "crash_after_ready";
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let ok = req.starts_with("GET /internal/ready ")
                    && req.contains(&format!("{TOKEN_HEADER}: {token}"))
                    && !req.to_ascii_lowercase().contains("content-type:")
                    && req.contains("\r\n\r\n");
                let _ = READY_PATH;
                let resp = if ok {
                    "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n"
                } else {
                    "HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n"
                };
                let _ = stream.write_all(resp.as_bytes());
                if crash && ok {
                    std::process::exit(9);
                }
                if !persist && ok {
                    hang();
                }
            }
            Err(_) => hang(),
        }
    }
}
