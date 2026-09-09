use super::*;

pub(super) fn wait_runtime_config(data: &Path, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if has_runtime_config(data) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for compiled config");
}

pub(super) fn has_runtime_config(data: &Path) -> bool {
    match fs::read_dir(data.join("runtime")) {
        Ok(rd) => rd
            .map(|entry| entry.expect("inspect runtime config entry"))
            .any(|e| {
                e.path()
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("config.") && n.ends_with(".bin"))
            }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => panic!("failed to inspect runtime config directory: {err}"),
    }
}

pub(super) fn assert_no_leaks(round: &Round, s3: &MockS3) {
    assert!(round.child.is_none());
    for pid in &round.tracked_pids {
        assert_gone(*pid, "tracked pid");
    }
    for port in &round.tracked_ports {
        assert!(
            TcpStream::connect(("127.0.0.1", *port)).is_err(),
            "port {port} still bound"
        );
    }
    let lock = round.data.join("platform.lock");
    if lock.exists() {
        assert!(
            open_compute_storage::DataDirLock::probe_available(&lock)
                .expect("lock probe must succeed"),
            "data dir lock still held"
        );
    }
    assert_no_partials(&round.data);
    assert!(
        !round.data.join("runtime/child.staging").exists(),
        "runtime staging journal leaked"
    );
    let leftover: Vec<_> = s3
        .keys()
        .into_iter()
        .filter(|k| k.contains("preflight") && k.contains(round.prefix.trim_end_matches('/')))
        .collect();
    assert!(leftover.is_empty(), "S3 canary leaked {leftover:?}");
    let _ = round.bind.as_str();
}

pub(super) fn assert_no_partials(root: &Path) {
    fn walk(path: &Path) {
        let rd = fs::read_dir(path)
            .unwrap_or_else(|err| panic!("failed to inspect {}: {err}", path.display()));
        for entry in rd {
            let e = entry.unwrap_or_else(|err| {
                panic!("failed to inspect an entry under {}: {err}", path.display())
            });
            let p = e.path();
            let name = e.file_name();
            let name = name.to_string_lossy();
            assert!(
                !name.contains(".partial") && !name.starts_with(".work") && name != ".tmp",
                "partial file leaked {}",
                p.display()
            );
            if p.is_dir() {
                walk(&p);
            }
        }
    }
    assert!(root.is_dir(), "expected data root {}", root.display());
    walk(root);
}

pub(super) fn read_lossy(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

pub(super) fn retain_failure(round: &Round) {
    let dest = repo_root().join(".temp/p0-1-run/failed").join(format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::create_dir_all(&dest);
    if let Ok(mut file) = fs::File::open(&round.stderr) {
        const MAX_RETAINED_STDERR: u64 = 64 * 1024;
        let len = file.metadata().map_or(0, |m| m.len());
        let _ = file.seek(SeekFrom::Start(len.saturating_sub(MAX_RETAINED_STDERR)));
        let mut raw = Vec::new();
        let _ = file.take(MAX_RETAINED_STDERR).read_to_end(&mut raw);
        let mut redacted = String::from_utf8_lossy(&raw)
            .replace("gate-secret-value", "[redacted]")
            .replace("gate-access", "[redacted]");
        for token in &round.known_tokens {
            redacted = redacted.replace(token, "[redacted-token]");
        }
        assert!(!redacted.contains("gate-secret-value"));
        assert!(!redacted.contains("gate-access"));
        assert!(
            round
                .known_tokens
                .iter()
                .all(|token| !redacted.contains(token))
        );
        let _ = fs::write(dest.join("stderr.log"), redacted);
    }
    let control = round.data.join("control.sqlite");
    let diagnostic_control = control
        .parent()
        .and_then(|parent| fs::canonicalize(parent).ok())
        .and_then(|parent| control.file_name().map(|name| parent.join(name)))
        .unwrap_or_else(|| control.clone());
    let opened = rusqlite::Connection::open_with_flags(
        &diagnostic_control,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    );
    let mut diagnostic = format!(
        "path_metadata={:?}\n",
        fs::symlink_metadata(&control)
            .map(|metadata| (metadata.len(), metadata.file_type().is_file()))
    );
    if let Ok(connection) = &opened {
        let version =
            connection.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0));
        diagnostic.push_str(&format!("user_version={version:?}\n"));
        match connection
            .prepare("SELECT key, typeof(value), length(value) FROM platform_meta ORDER BY key")
        {
            Ok(mut statement) => match statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            }) {
                Ok(rows) => {
                    for row in rows {
                        diagnostic.push_str(&format!("meta={row:?}\n"));
                    }
                }
                Err(error) => diagnostic.push_str(&format!("meta_query={error:?}\n")),
            },
            Err(error) => diagnostic.push_str(&format!("meta_prepare={error:?}\n")),
        }
    } else {
        diagnostic.push_str(&format!("open={opened:?}\n"));
    }
    let _ = fs::write(dest.join("database-diagnostic.log"), diagnostic);
}
