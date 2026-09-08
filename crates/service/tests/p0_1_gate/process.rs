use super::*;

pub(super) fn required_workerd() -> PathBuf {
    let path = std::env::var("OPEN_COMPUTE_TEST_WORKERD").unwrap_or_default();
    assert!(
        !path.is_empty(),
        "OPEN_COMPUTE_TEST_WORKERD is required; refusing to skip the Gate"
    );
    let p = PathBuf::from(path);
    assert!(
        p.is_absolute(),
        "OPEN_COMPUTE_TEST_WORKERD must be absolute"
    );
    assert!(p.is_file(), "OPEN_COMPUTE_TEST_WORKERD is not a file");
    p
}

pub(super) fn verify_workerd(lock: &RuntimeLock, binary: &Path) {
    let target = lock.current_target().expect("current target in lock").1;
    let bytes = fs::read(binary).expect("read workerd");
    let digest = hex::encode(Sha256::digest(&bytes));
    assert_eq!(
        digest, target.binary_sha256,
        "test workerd hash must match the formal lock"
    );
}

pub(super) fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

pub(super) fn http_status(port: u16, path: &str) -> Option<u16> {
    http_get(port, path).map(|(c, _)| c)
}

pub(super) fn http_get(port: u16, path: &str) -> Option<(u16, String)> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let auth = if path.starts_with("/client/v4/") || path == "/metrics" {
        format!("Authorization: Bearer {ADMIN_TOKEN}\r\n")
    } else {
        String::new()
    };
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth}Connection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).ok()?;
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let code = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())?;
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    Some((code, body))
}

pub(super) fn probe_workerd(port: u16, token: &str) -> bool {
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
        return false;
    };
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{}: {token}\r\nConnection: close\r\n\r\n",
        open_compute_runtime::READY_PATH,
        open_compute_runtime::TOKEN_HEADER
    );
    let _ = stream.write_all(req.as_bytes());
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    text.contains("204") || text.contains("200")
}

pub(super) fn listen_ports(pid: i32) -> Vec<u16> {
    let out = Command::new("/usr/sbin/lsof")
        .args(["-nP", "-p", &pid.to_string(), "-a", "-iTCP", "-sTCP:LISTEN"])
        .output()
        .or_else(|first| {
            Command::new("lsof")
                .args(["-nP", "-p", &pid.to_string(), "-a", "-iTCP", "-sTCP:LISTEN"])
                .output()
                .map_err(|_| first)
        })
        .expect("lsof is required by the process Gate");
    let code = out.status.code();
    assert!(
        matches!(code, Some(0 | 1)),
        "lsof failed for pid {pid}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    let mut ports = Vec::new();
    for line in text.lines().filter(|line| line.contains("(LISTEN)")) {
        let fields: Vec<_> = line.split_whitespace().collect();
        let Some(listen_index) = fields.iter().position(|field| *field == "(LISTEN)") else {
            continue;
        };
        let Some(address) = listen_index
            .checked_sub(1)
            .and_then(|index| fields.get(index))
        else {
            continue;
        };
        let Some((_, port)) = address.rsplit_once(':') else {
            continue;
        };
        if let Ok(port) = port.parse::<u16>()
            && port != 0
            && !ports.contains(&port)
        {
            ports.push(port);
        }
    }
    ports
}

pub(super) fn health_port_from(ports: &[u16]) -> Option<u16> {
    ports
        .iter()
        .copied()
        .find(|&port| http_status(port, "/health/live") == Some(200))
}

pub(super) fn public_health_port(pid: i32) -> Option<u16> {
    health_port_from(&listen_ports(pid))
}

pub(super) fn sole_listen_port(pid: i32) -> Option<u16> {
    let ports = listen_ports(pid);
    match ports.as_slice() {
        [] => None,
        [port] => Some(*port),
        _ => panic!("pid {pid} has multiple listener ports: {ports:?}"),
    }
}

pub(super) fn respond_once(listener: TcpListener, status: u16) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept health probe");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set probe read timeout");
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request);
        let reason = if status == 200 { "OK" } else { "Not Found" };
        write!(
            stream,
            "HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .expect("write health response");
    })
}

pub(super) fn staged_executable(pid: i32) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let out = Command::new("lsof")
            .args(["-p", &pid.to_string(), "-a", "-d", "txt", "-Fn"])
            .stdin(Stdio::null())
            .output()
            .expect("lsof is required by the process Gate");
        assert!(out.status.success(), "lsof failed for workerd pid {pid}");
        let text = String::from_utf8(out.stdout).expect("lsof output must be UTF-8");
        text.lines()
            .filter_map(|line| line.strip_prefix('n'))
            .map(PathBuf::from)
            .find(|path| {
                path.file_name().is_some_and(|name| name == "workerd")
                    && path
                        .parent()
                        .and_then(Path::file_name)
                        .is_some_and(|name| name.to_string_lossy().starts_with("oc-exec-"))
            })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        None
    }
}

pub(super) fn staging_directories() -> BTreeSet<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        fs::read_dir(std::env::temp_dir())
            .expect("inspect temporary directory")
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("oc-exec-"))
            .map(|entry| entry.path())
            .collect()
    }
    #[cfg(not(target_os = "macos"))]
    {
        BTreeSet::new()
    }
}

pub(super) fn child_pids(parent: i32) -> Vec<i32> {
    let out = Command::new("pgrep")
        .args(["-P", &parent.to_string()])
        .output()
        .expect("pgrep is required by the process Gate");
    match out.status.code() {
        Some(0) => {}
        Some(1) => return Vec::new(),
        code => panic!(
            "pgrep failed for parent {parent} with {code:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        ),
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|line| {
            line.trim()
                .parse()
                .unwrap_or_else(|_| panic!("pgrep returned a non-PID line: {line:?}"))
        })
        .collect()
}

pub(super) fn pid_alive(pid: i32) -> bool {
    let raw = Pid::from_raw(pid).expect("tracked PID must be positive");
    match test_kill_process(raw) {
        Ok(()) => true,
        Err(err) if err == rustix::io::Errno::SRCH => false,
        Err(err) => panic!("failed to probe pid {pid}: {err}"),
    }
}

pub(super) fn assert_gone(pid: i32, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while pid_alive(pid) {
        if Instant::now() >= deadline {
            panic!("{what} pid {pid} still live after wait deadline");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn note_tree(round: &mut Round, pid: i32) {
    round.tracked_pids.push(pid);
    for c in child_pids(pid) {
        round.tracked_pids.push(c);
        round.tracked_ports.extend(listen_ports(c));
    }
    round.tracked_ports.extend(listen_ports(pid));
}

pub(super) fn assert_round_preflight(s3: &MockS3, prefix: &str) {
    let rec: Vec<_> = s3
        .recorded()
        .into_iter()
        .filter(|r| r.path.contains(prefix.trim_end_matches('/')) && r.path.contains("preflight"))
        .collect();
    let methods: Vec<_> = rec.iter().map(|r| r.method.as_str()).collect();
    assert_eq!(
        methods,
        ["PUT", "HEAD", "HEAD", "GET", "DELETE", "HEAD"],
        "round prefix {prefix} methods {methods:?}"
    );
    assert!(rec.iter().all(|r| r.has_authorization));
    let leftover: Vec<_> = s3
        .keys()
        .into_iter()
        .filter(|k| k.contains("preflight") && k.contains(prefix.trim_end_matches('/')))
        .collect();
    assert!(leftover.is_empty(), "canary left behind {leftover:?}");
}

pub(super) fn kill_tree(pid: i32) {
    for c in child_pids(pid) {
        kill_tree(c);
    }
    if let Some(raw) = Pid::from_raw(pid) {
        let _ = kill_process(raw, Signal::KILL);
    }
}

pub(super) fn extract_token(data: &Path, lock: &RuntimeLock, runtime_port: u16) -> Option<String> {
    let skip: Vec<String> = lock
        .targets
        .values()
        .flat_map(|t| [t.binary_sha256.clone(), t.archive_sha256.clone()])
        .collect();
    let runtime = data.join("runtime");
    let entries = fs::read_dir(runtime).ok()?;
    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("bin")
            && let Ok(bytes) = fs::read(&path)
        {
            let text = String::from_utf8_lossy(&bytes);
            for c in extract_hex64(&text) {
                if !skip.iter().any(|s| s == &c) && !candidates.contains(&c) {
                    candidates.push(c);
                }
            }
        }
    }
    candidates
        .into_iter()
        .find(|c| probe_workerd(runtime_port, c))
}

pub(super) fn extract_hex64(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 64 <= bytes.len() {
        if bytes[i..i + 64]
            .iter()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            let s = String::from_utf8_lossy(&bytes[i..i + 64]).into_owned();
            if !out.contains(&s) {
                out.push(s);
            }
            i += 64;
        } else {
            i += 1;
        }
    }
    out
}

pub(super) fn assert_token_absent(
    token: &str,
    platform: i32,
    workerd: i32,
    status: &str,
    metrics: &str,
    stderr: &Path,
) {
    assert!(!status.contains(token));
    assert!(!metrics.contains(token));
    assert!(!read_lossy(stderr).contains(token));
    for pid in [platform, workerd] {
        let out = Command::new("/bin/ps")
            .args(["eww", "-p", &pid.to_string()])
            .output()
            .expect("/bin/ps must succeed to assert token absence");
        assert!(
            out.status.success(),
            "/bin/ps failed for pid {pid}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(!text.contains(token), "token leaked in process listing");
    }
}

pub(super) fn platform_id(data: &Path) -> String {
    let raw = fs::read(data.join("platform.lock")).expect("lock metadata");
    let v: serde_json::Value = serde_json::from_slice(&raw).expect("lock json");
    v.get("platform_id")
        .and_then(|x| x.as_str())
        .expect("platform_id")
        .to_owned()
}

pub(super) fn wait_path(path: &Path, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {}", path.display());
}
