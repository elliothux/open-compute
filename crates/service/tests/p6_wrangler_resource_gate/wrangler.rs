use super::*;

pub(super) struct WranglerCommand<'a> {
    pub(super) executable: PathBuf,
    pub(super) project: &'a Path,
    pub(super) api_base_url: String,
    pub(super) account_id: &'a str,
}

impl WranglerCommand<'_> {
    pub(super) fn command(&self, args: &[&str]) -> tokio::process::Command {
        assert!(self.api_base_url.starts_with("http://127.0.0.1:"));
        let mut command = tokio::process::Command::new(&self.executable);
        command
            .args(args)
            .current_dir(self.project)
            .env("CLOUDFLARE_API_BASE_URL", &self.api_base_url)
            .env("CLOUDFLARE_API_TOKEN", TOKEN)
            .env("CLOUDFLARE_ACCOUNT_ID", self.account_id)
            .env("WRANGLER_SEND_METRICS", "false")
            .env("WRANGLER_SEND_ERROR_REPORTS", "false")
            .env("WRANGLER_NO_SKILLS_UPDATE_PROMPTS", "true")
            .env("WRANGLER_HIDE_BANNER", "true")
            .env("DO_NOT_TRACK", "1")
            .env("CI", "true")
            .env("XDG_CONFIG_HOME", self.project.join("xdg"))
            .env("HTTP_PROXY", "http://127.0.0.1:9")
            .env("HTTPS_PROXY", "http://127.0.0.1:9")
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env_remove("CF_API_BASE_URL")
            .env_remove("CLOUDFLARE_BASE_URL")
            .env_remove("CLOUDFLARE_API_KEY")
            .env_remove("CLOUDFLARE_EMAIL")
            .kill_on_drop(true);
        command
    }

    pub(super) async fn run(&self, args: &[&str]) -> Output {
        let mut command = self.command(args);
        tokio::time::timeout(Duration::from_secs(60), command.output())
            .await
            .expect("fixed Wrangler resource command timed out")
            .expect("fixed Wrangler and Node.js must already be installed")
    }

    pub(super) fn spawn(
        &self,
        args: &[&str],
        stdout: &Path,
        stderr: &Path,
    ) -> tokio::process::Child {
        let mut command = self.command(args);
        command
            .env_remove("HTTP_PROXY")
            .env_remove("HTTPS_PROXY")
            .env_remove("ALL_PROXY")
            .env_remove("http_proxy")
            .env_remove("https_proxy")
            .env_remove("all_proxy")
            .stdout(Stdio::from(fs::File::create(stdout).unwrap()))
            .stderr(Stdio::from(fs::File::create(stderr).unwrap()))
            .spawn()
            .expect("spawn fixed Wrangler tail")
    }
}

pub(super) fn fixed_wrangler() -> PathBuf {
    let root = repo_root();
    let lock = fs::read_to_string(root.join("bun.lock")).unwrap();
    assert!(lock.contains("\"wrangler\": [\"wrangler@4.127.1\""));
    let prefix = format!("wrangler@{WRANGLER_VERSION}+");
    let mut installs = fs::read_dir(root.join("node_modules/.bun"))
        .expect("locked Bun dependencies must already be installed")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    installs.sort();
    assert_eq!(
        installs.len(),
        1,
        "exactly one fixed Wrangler must be installed"
    );
    let package = installs[0].join("node_modules/wrangler");
    let metadata: Value =
        serde_json::from_slice(&fs::read(package.join("package.json")).unwrap()).unwrap();
    assert_eq!(metadata["version"], WRANGLER_VERSION);
    package.join("bin/wrangler.js")
}

pub(super) fn assert_success(output: &Output) {
    assert_clean_output(&output.stdout);
    assert_clean_output(&output.stderr);
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

pub(super) fn assert_clean_output(bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    for secret in evidence::known_secrets() {
        assert!(!text.contains(secret));
    }
    assert!(!text.contains("api.cloudflare.com"));
}

pub(super) fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "Wrangler JSON output was invalid: {error}: {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

pub(super) fn json_contains(value: &Value, key: &str, expected: &Value) -> bool {
    match value {
        Value::Object(object) => {
            object.get(key) == Some(expected)
                || object
                    .values()
                    .any(|value| json_contains(value, key, expected))
        }
        Value::Array(values) => values
            .iter()
            .any(|value| json_contains(value, key, expected)),
        _ => false,
    }
}

pub(super) fn storage_config(root: &Path) -> DataConfig {
    DataConfig {
        path: root.to_owned(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 1,
    }
}

pub(super) fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}
